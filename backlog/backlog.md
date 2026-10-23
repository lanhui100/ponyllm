# Backlog（轻量模式）

> 本仓库无 taskctl/openspec 基建，采用轻量 backlog（事项级）。执行级步骤见各会话。

## 进行中

（无）

## 待办

- **B001** [P1] antigravity 额度管理面：刷新策略与前端矩阵方块显示偏差对齐；含
  `.agents/notes/proposed/architecture/2026-09-10-antigravity-midstream-quota-frame-key-cooldown.md`
  （200-SSE 帧内配额冷冻）落地。→ 单开会话 A
- **B002** [P1→已完成 2026-10-02] 升级/重启导致 dashboard 数据从 0 开始：统一计量内核持久化已落地
  （SCHEMA_VERSION=2 + key_usage_cycles 归档接线）。本轮完成：快照 key_usages 读-改-写合并
  （不在册 key 历史保留）+ 热重载按 key id 移植 usage tracker + schema v3 池级跨账号跨周期
  持久化累计基准（`pool_cycle_benchmark`，水位线幂等合并）+ `GET /api/admin/quota/benchmark`
  端点 + dashboard 第三栏改读持久化基准。验收：
  `cargo test -p ponyllm-core -p ponyllm-server`（含 hot_reload_usage_tests /
  quota_benchmark_api_tests / telemetry_snapshot）+ `cd web && pnpm test` 全绿。
  集群侧（dev 副本 PVC 快照跨发版延续）靠 review：`kubectl exec deploy/ponyllm-gateway-dev -- ls -la /var/lib/ponyllm/telemetry-snapshot.json`

## 完成

- **B003**（2026-10-17）`/v1/models` 容量字段 → 已实施：列表与单模型详情新增 `context_window` /
  `max_output`（13 键），每模型按 `canonicalize_model_name` 同名组（default_model ∪ models 的
  `get_model_spec`）取分项 max，别名/多段名/gemini 变体不回落默认 ModelSpec，auto 取全局 max；
  `list_all_models()` 签名与逐元素行为不变。ADR:
  `.agents/notes/implemented/feature/2026-10-17-b003-model-capacity-fields.md`。验收：冻结契约
  CAP-01..15 / J-01..06 全绿（21/21）、`cargo test -p ponyllm-server` 569 passed、真实二进制
  `ponyllm serve` + curl `/v1/models` 断言通过。

- **B002**（2026-10-02）升级/重启导致 dashboard 数据从 0 开始 → 已修复：快照 key_usages 合并保留、
  热重载移植 usage tracker、schema v3 池级跨账号跨周期持久化累计基准 + benchmark 端点 + 前端接入。
- **统一账户额度计量内核**（2026-09-30，本轮）：短窗计量 + 持久化迁移 + rate_limits 配置面
  + 调度消费 + 透明等待。ADR: `.agents/notes/implemented/architecture/2026-09-30-unified-quota-metering-governance-kernel.md`

## DeepSeek Web Provider（deepseek-web）— waves 9–12 已交付（2026-10-22/23）

- **wave-9** chat.deepseek.com 合规接入：PoW（DeepSeekHashV1 实测真值：prefix=salt_expireAt、23 轮 Keccak）、会话/SSE/翻译、错误映射、合规边界。
- **wave-10** 图片输入桥接：OpenAI `image_url`/Anthropic `type:image` → SSRF-safe 上传 → `ref_file_ids`；SSRF 零外发；`/v1/messages` 支持 deepseek-web。
- **wave-11** 工具调用模拟 + 多轮角色：`[system]/[user]/[assistant]/[tool:<name>]`；schema 注入 → `tool_calls`/`tool_use`；失败显式化（禁静默）。
- **wave-12** 思考系统化：开关四来源统一、effort 映射、history `reasoning_content` 注入（≤4KB 尾部截断）、非泄漏/流式顺序。
- **生产**：4 节点已滚动部署（wave-12 镜像 `sha256:d97098cdbf2b`）；`dompurify 3.4.17` 安全升级。
- **文档**：ADR `.agents/notes/implemented/architecture/2026-10-22-deepseek-web-{provider,waves-10-12}.md`；运行手册 `docs/deepseek-web-provider.md`；README 挂载示例。
- **验收**：R-VIS 23、R-MT/TOOL 28、R-THINK 14 全绿（`--test-threads=4`）；真实 E2E（识图 Red、工具循环、thinking 开关）本地+生产通过。

## 待办（新会话 REA 主题）

- **deepseek-web token 获取/续期研究**：当前凭据为浏览器手动取的 web token（长期有效，无 cookie→token 自动续期路径——已实测 cookie 不参与 API 鉴权、无 refresh 端点）。研究目标：可否以手机验证码登录（15538643833）换取/续期 token 的合规自动化路径。登录本身可用验证码；不触碰验证码/WAF 绕过边界。
- **生产 thinking 专项验收**：需有效 token（旧 token 已失效 40003）后跑 `docs/deepseek-web-provider.md §4` 验证命令。
- 可选：文档附件（非图片 file_id 桥接）；`expert` 上游启用后收敛别名。
