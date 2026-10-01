# Agent Note: 配置存储访问超时熔断与读写分离设计

Status: implemented

## Problem

在 Kubernetes 托管的微服务架构中，`ponyllm` 作为高性能 LLM 转发网关，在多节点高可用模式下依赖 Kubernetes Secret（如 `ponyllm-live-config`）作为动态配置真相源。
在一次边缘节点与公网跨地域部署场景中，腾讯云 K3s Master 节点发生假死宕机事故。由于底层的 `kube-rs` / `hyper` 客户端在没有显式请求级超时护栏的情况下，发起网络连接和数据请求会继承底层 TCP 协议栈长达 30 秒至 127 秒的无响应死等。
这导致两类严重后果：
1. 后台轮询器 `ConfigPoller` 线程阻塞挂起，无法快速感知失败并实施降级；
2. 管理员执行 `Admin API` 时连接长时间挂起，并且若写操作（`PATCH`）在边缘端超时而在远端 etcd 异步落盘，将造成读写双向状态不一致（脑裂风险）。

## Decision

1. **显式超时熔断护栏**：
   在 `crates/ponyllm-server/src/admin_store.rs` 中的 `KubeSecretApi` 的底层 `get` 与 `patch_data` 请求外层显式包裹 `tokio::time::timeout` 护栏，无论 TCP/TLS 握手、慢速连接（Slowloris）还是远端挂起，均在预设阈值内被强制取消并释放资源。
2. **读写超时分离**：
   - **读操作（GET）**：默认阈值设为激进的 `1500ms`（`DEFAULT_KUBE_READ_TIMEOUT`），配合环境变量 `PONYLLM_KUBE_READ_TIMEOUT_MS`，实现后台 Poller 和查询接口的超快速脱困与零中断降级。
   - **写操作（PATCH）**：默认阈值放宽至 `5000ms`（`DEFAULT_KUBE_WRITE_TIMEOUT`），配合环境变量 `PONYLLM_KUBE_WRITE_TIMEOUT_MS`，为跨地域 etcd 多副本 Quorum 复制提供合理等待窗口，规避 Future cancellation 带来的误报脑裂风险。
   - 支持向后兼容旧变量 `PONYLLM_KUBE_TIMEOUT_MS`，统一加入 `clamp(100ms, 30_000ms)` 防御性数值区间护栏，并对非法格式输出 `tracing::warn!`。
3. **结构化错误模型与 HTTP 脱敏**：
   - `ConfigStoreError::Timeout { operation: &'static str, duration: Duration }` 提供精确的机器可读与结构化观测支持。
   - `Admin API` 在拦截到 `Timeout` 错误时，统一返回脱敏的 HTTP 504 响应码与错误文案 `"config store operation timed out (control plane offline)"`，避免泄露内部 Kubernetes 命名空间与 Secret 名称等拓扑信息。
4. **零堆分配优化**：
   在 `KubeSecretApi` 实例化时直接固化 `kube::Api<Secret>` 句柄，消除每次读写重复构造及克隆 `Arc/String` 的微小堆分配开销。

## Alternatives considered

- **完全依赖 kube-rs Client 的全局底层超时配置**：
  kube-rs 的 ClientBuilder 虽支持超时设置，但全局统一超时无法做到读写操作的粒度分离（读要求 1.5s 激进脱困，写需要 5.0s 防 etcd 复制延迟）；并且无法针对性捕获 `ConfigStoreError::Timeout` 进行脱敏及精准映射 HTTP 504。
- **由上层调用方（如 admin route 与 config poller）各自包裹 timeout**：
  导致超时逻辑分散在各个路由和任务函数中，破坏了 `admin_store` 作为独立存储层抽象的内聚性，且容易遗漏测试覆盖。
- **对写超时同样设为 1.5s**：
  经对抗审查确认，跨地域 etcd 写入如果超过 1.5s，客户端抛出超时但远端可能已成功写入，外部重试将触发 412 CAS 冲突，故必须将写超时放宽至 5.0s。

## Consequences

- 网关在 K8s 控制面宕机、丢包黑洞或跨地域断网时，读操作 1.5 秒内必返回，写操作 5 秒内必返回，彻底消除了连接挂死隐患。
- HTTP 响应脱敏，防止集群拓扑暴露。
- 底层连接在 Future 丢弃时被 Hyper 干净断开，无 socket/fd 泄漏。
