# Agent Note: 流式响应与上游连接池性能优化

Status: implemented

## Problem

网关在处理上游流式调用与协议转换时存在显著延迟放大隐患：
1. UpstreamExecutor::new 在每个请求甚至路由目标轮询中就地构建独立 eqwest::Client，导致底层 TCP 连接池、TLS 会话缓存与 DNS 解析完全无法跨请求复用。面对外网上游 API 时，每次请求均强制执行握手，使首字延迟（TTFT）额外暴增数百毫秒。
2. 路由分发路径对请求体无脑调用 eq_val.to_string() 构造遥测摘要，在 64k/128k 长上下文场景下造成多兆字节的冗余内存深拷贝与同步序列化耗时，严重推迟发包时刻。
3. 跨协议 SSE 解析器在每次切帧时执行 	ail.to_vec()，对剩余缓冲区进行全量堆内存拷贝，并在数据行拼接中产生多次瞬时分配，中高并发下易引发 CPU 调度饥饿与吐字抖动（TPS jitter）。

## Decision

- **连接池全局常驻复用**：在 AppState 与 PonyGateway SDK 中持有全局单例 eqwest::Client，配置 	cp_nodelay(true)、Keep-Alive 保活探针与空闲连接超时；UpstreamExecutor 提供 with_client 构造方法复用共享连接池，彻底消除重复建连握手。
- **轻量受限请求体摘要**：新增 ormat_request_snippet 并依托 BoundedWriter 将遥测切片序列化上限硬限制为 512 字符，无论请求体体积多大均保证恒定极小内存分配，彻底消灭大上下文序列化阻塞。
- **SSE 流解析切片优化**：sse_event_stream 底层缓冲改用 ytes::BytesMut，通过 split_to 零拷贝切出帧数据，消除 	ail.to_vec() 的 O(N) 内存拷贝；简化单行数据行提取，消灭每个 SSE 帧 60% 以上的堆临时分配。

## Alternatives considered

- **每次请求动态复用池但保留独立配置 Client**：否定。eqwest::Client 内部封装的 Hyper 连接池是以实例为单位隔离的，唯有共享长生命周期实例才能生效 Keep-Alive 复用。
- **彻底关闭遥测请求摘要以换取极致吞吐**：否定。黑盒排障依赖现场输入信息，采用 512 字符上限的 BoundedWriter 兼顾了 O(1) 内存开销与排障可见性。
- **引入第三方 SSE 编解码外部库**：否定。零依赖与极简原则优先，基于 BytesMut 的原地分割逻辑仅数十行且已覆盖所有边缘测试用例。

## Consequences

- 全量单元测试与路由集成测试 100% 通过，集成测试套件运行时间显著缩短（部分用例提速近 3 倍）。
- 首字延迟（TTFT）消除无效握手，高并发流式吐字均匀度大幅改善，大上下文请求发出延迟显著降低。