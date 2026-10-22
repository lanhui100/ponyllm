# Agent Note: deepseek-web Provider（chat.deepseek.com 登录态 Web 会话接入网关）

Status: implemented

## Problem

用户要求基于 REA（被动取证 chat.deepseek.com 公开前端资产）结论，将 chat.deepseek.com Web 对话接口接入 ponyllm 网关为可调用 Provider：以用户自持登录态 token 为凭据，按官方客户端同等的协议方式（客户端头 + PoW 挑战 + SSE 流）调用。必须严守合规边界：不绕过 WAF/验证码/设备指纹，不做多账号轮换、不规避配额与频控、PoW 每次诚实单解。

## Decision

新增 `UpstreamProtocol::DeepSeekWeb` provider 形态（wave-9）：

1. **客户端模块** `crates/ponyllm-core/src/deepseek_web/`：DeepSeekHashV1 PoW 求解器、challenge 获取/应答头编码、会话与 completion 客户端、SSE 事件解析（ready/delta/finish/close，delta 为 JSON-patch ops o/p/v 含 BATCH）。
2. **PoW 协议（本波最重要的执行真值修正）**：以官方 worker（`static/37627.*.js` / `76608.*.js`）可执行结果为唯一权威——**prefix = `salt + "_" + expire_at + "_"`（不是 REA 初判的 salt_signature）**；哈希 = **Keccak-f[1600] SHA3-256 参数但仅 23 轮 iota**（24 轮变体严格等于标准 SHA3-256）。依据：worker 闭包调用 `((t,e,r,n,i)=>{...})(e,r,n,i,s)` 形参重排、worker 报错消息自曝 `prefix: abc_12345_`、salt_signature 假设 challenge 在 difficulty 内无解（Lead 独立驱动 worker 复核 + exec-client Node shim 双证）。golden vectors 全部改为 worker 驱动生成。
3. **网关集成**：`chat.deepseek.com` base_url 启发式 → DeepSeekWeb；executor 分支完成 session→challenge→solve→`X-DS-PoW-Response` 附头 → 走既有 `send_guarded`/TTFB 守卫（90s/1200s 常量不调参）；错误映射 40012/40002/40003→AuthInvalid（key 隔离），其余→标准瞬时 failover；SSE→OpenAI chunk 翻译器（thinking→reasoning_content，close 错误帧显式透传，DW-T-06 降级按 `emitted_content_len` 游标只补增量）。
4. **合规硬边界**：不实现验证码/滑块/指纹绕过（`x-amzn-waf-action: captcha` / `cf-mitigated: challenge` → 显式 `ClientError::CaptchaWall` 零重试）；无游客端点；429 按 Retry-After 有界退避；PoW 单次诚实求解、无重放无预批量。
5. **凭据形态**：`keys[].api_key` 存用户自持 web token，零新增配置键；CLI 向导新增 deepseek-web 模板。

## Alternatives considered

- **按 REA 初判实现 salt+"_"+signature+"_" 前缀 + 标准 SHA3-256**：被否。直接驱动官方 worker 证明该假设构造的 challenge 在 difficulty 内无解；worker 报错自曝真实 prefix 含 expireAt。契约以可执行真值为准（REA §1.2 自证"以 worker 为准做差分验证，不得仅凭推导"），走契约演进门禁出 Amendment v2。
- **将 SSE→OpenAI 翻译器冻结为 core 函数签名**：被否。翻译落地在 server 侧 chat.rs（参照 antigravity_sse_to_openai_stream 模式），core 只冻结类型化事件模型，避免 core/server 双实现漂移。
- **游客模式/stop_stream/多端点**：被否。无 token 时显式报错不转游客（DW-S-05）；stop_stream 语义未验证，本波不实现（§9.3）。
- **流式 3000ms 豁免**：被否。SSE 首字节沿用既有 `DEFAULT_UPSTREAM_TTFB_TIMEOUT=90s`（upstream.rs:1074），3000ms 仅约束短调用（challenge/会话），不新增豁免参数。

## Consequences

- 用户持 chat.deepseek.com web token 可经网关以 OpenAI 兼容协议调用 deepseek-web 模型；PoW 挑战按官方协议诚实求解，每次请求单解。
- 网关侧新增一个协议变体，所有 `UpstreamProtocol` 穷举 match 点需带 DeepSeekWeb 分支（已全量补全并编译验证）。
- 合规边界以契约 DW-G-* 冻结并有对应测试（WAF 两例零重试、429 有界退避、无游客端点静态扫描）；遇到上游风控升级（如强制验证码）客户端显式失败提示人工处理，不做自动绕过。
- 已知限制（契约 §10）：finish usage 字段"待集成验证"，缺失时省略不伪造；close.close_reason 缺失按 success；delta path 语义歧义时走 DW-T-06 降级（final 内容一次输出）。
