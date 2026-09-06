# Agent Note: Model-level proxy override and dynamic proxy auto-detection

Status: implemented

## Problem

此前网关网络层解除了宿主终端代理环境变量对网关的污染（统一默认直连），并支持在 Provider 级别配置 `proxy` 属性。
但在真实使用场景中存在更细粒度的网络分流需求：
1. **多模型差异化路由**：同一 Provider（如 `opencode-zen`）下的大多数模型（如 deepseek-chat 等）国内直连速度最快，无需代理；而个别模型（如 `muse-spark` 或特定海外端点模型）则必须通过本地 HTTP/SOCKS 代理访问。现有架构中 Provider 级别的 proxy 是一刀切的，无法满足同一提供商下不同模型的差异化路由。
2. **端口硬编码风险与配置门槛**：用户在通过向导（`ponyllm init`）或命令行添加 Provider/Model 时，若需要配置代理，必须手动记住并输入端口。若代码中硬编码默认代理端口（如 8899 或 7890），一旦用户环境代理端口变动或运行在不同机器上，极易造成请求失败。

## Decision

1. **动态系统代理检测 (`detect_system_proxy`)**：
   - 探测层级按优先级依次执行：
     1. 读取环境变量：`HTTPS_PROXY` / `https_proxy` / `HTTP_PROXY` / `http_proxy` / `ALL_PROXY` / `all_proxy`；
     2. 检查用户配置导出文件 `~/.pony/proxy.env`（解析 `export http_proxy=` 或 `export https_proxy=`）；
     3. 探测本机常见代理监听端口（如 8899, 7890, 10808, 10809, 8080）是否活跃（TCP 快速握手，超时 30ms）。
   - 探测结果提供给 CLI 交互向导与命令行参数 `--proxy auto` 作为候选值，避免任何写死代码。

2. **数据契约与继承/覆盖语义**：
   - 在 `ModelSpec` (Server) 与 `ModelConfig` (CLI) 中增加可选字段 `proxy: Option<String>`。
   - 继承与覆盖规则：
     - `spec.proxy == None`：继承所属 Provider 的 `proxy` 设置；
     - `spec.proxy == Some("direct")` 或 `Some("none")`：强制直连，即使 Provider 配了代理也被覆盖；
     - `spec.proxy == Some(url)`：使用指定的代理 URL。
   - 同理，Provider 的 `proxy` 继承 Gateway 级默认设置，或者为 None（直连）。

3. **网关连接池与按目标路由 (`http_client_for_target`)**：
   - `AppState` 内部维护以有效代理 URL 字符串为键的共享 HTTP 客户端缓存 `proxy_clients: RwLock<HashMap<String, reqwest::Client>>`，直连请求使用 `direct_client`。
   - 相同代理端点的请求跨模型、跨提供商复用底层连接池与 TCP Keep-Alive，最小化 TLS 握手与 TTFT 开销。
   - `chat`、`responses`、`messages` 路由入口统一基于 `(provider_name, physical_model)` 解析有效代理并派发对应客户端。

4. **CLI 与交互式向导升级**：
   - `ponyllm init` 向导增加“是否为该提供商配置代理”提问，默认 `N`（直连）；若选 `Y` 则调用 `detect_system_proxy()` 填充默认值供用户确认或微调。
   - `ponyllm provider add` 增加 `--proxy <URL|auto|none>`。
   - `ponyllm model add` 增加 `--proxy <URL|auto|direct|none>`。
   - `ponyllm model list` 在表格中清晰输出代理状态（继承或独立覆盖）。

## Alternatives considered

- **方案 A：仅支持 Provider 级代理，要求拆分为两个 Provider**
  - *分析*：如把 `opencode-zen` 拆为 `opencode-zen-direct` 和 `opencode-zen-proxy` 两个 Provider 配置。
  - *缺点*：割裂了同一个上游提供商的账户池（API Key 池无法共享、额度和限流状态分散），且污染模型命名空间，增加了客户端配置负担。
- **方案 B：硬编码常见代理端口作为默认值**
  - *分析*：如在向导中写死 `http://127.0.0.1:8899`。
  - *缺点*：在不同环境（Clash 默认为 7890，v2ray 为 10808，极简环境可能无代理）下体验极差，甚至诱导用户配置错误代理。动态探测并允许用户确认是唯一的健壮方案。
- **方案 C：为每个 Model 每次请求临时构建 `reqwest::Client`**
  - *分析*：每次根据 model 的 proxy 临时 `Client::builder().build()`。
  - *缺点*：完全破坏了 HTTP 连接池复用，每次请求都需要重新做 TCP 握手和 TLS 协商，严重恶化首字时延（TTFT）。按有效代理 URL 缓存连接池是高性能网关的唯一正确选择。

## Consequences

- 满足了用户对同一 Provider 下不同模型（如 `opencode-zen` 的直连模型与 `muse-spark` 代理模型）精细化分流的需求。
- 零硬编码：在无配置时保持默认直连，在需要代理时自动发现环境现有端口并建议，用户也可显式覆盖。
- 保持了极致的网络性能：通过 URL 级客户端缓存池，相同代理目标的连接保持 TCP Keep-Alive 复用，避免频繁建立连接开销。
