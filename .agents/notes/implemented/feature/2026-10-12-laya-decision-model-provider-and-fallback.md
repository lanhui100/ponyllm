# Feature ADR: Laya 决策模型接入与 SystemOne 自动 Fallback 链路

Status: implemented

## Decision

1. **别名规范化 (`crates/ponyllm-core/src/endpoints.rs`)**：
   - 在 `canonicalize_model_name` 中将 `laya` 与 `laya-free` 规范化为 `convaiinnovations/laya`；
   - 在 `model_aliases` 中将 `convaiinnovations/laya` 关联别名 `["laya", "laya-free"]`。
2. **Provider 配置 (`vercel`)**：
   - `base_url`: `https://ai-gateway.vercel.sh/typesafe/v1`（`normalize_systemone_url` 自动拼接为 `/systemone` 原生路径）；
   - `default_protocol`: `systemone`；
   - `default_model`: `convaiinnovations/laya`；
   - `billing_mode`: `free`；
   - 经由 Admin API 注册并同步至 Kubernetes 集群配置及本地配置。
3. **Fallback 降级映射**：
   - 在 `zen-jev` 的 `jev-1.13-free` model_configs 中配置 `fallbacks = ["convaiinnovations/laya"]`；
   - 当 `jev-1.13-free` 报错 429/配额耗尽时，路由层透明切换至 `convaiinnovations/laya`。
4. **Skill 文档与降级指导**：
   - 更新 `skills/jev-agent-skill/SKILL.md`，记录网关自动降级与客户端主动调用规格，并同步更新 user-agents 层安装副本。

## Alternatives considered

- **方案 A：在客户端维护重试与降级**：
  - 缺点：每个调用 Agent 必须自行解析 429/403 异常、实现轮转逻辑与多 Provider 鉴权，增加了客户端复杂性。
  - 结论：否决。网关层内建故障倒换最优雅、对应用层完全透明。
- **方案 B：将 Laya 接入为 OpenAI Decisions API 协议 (`/v1/decisions`)**：
  - 缺点：OpenAI Decisions API 采用数组形式 questions 与 `predicate` 命名，与现有 Jev / System One 的 `state` + 对象映射 + `noul` 原语不兼容，需要双向协议转换。
  - 结论：否决。Vercel AI Gateway 已经提供原生的 `/typesafe/v1/systemone` 端点，直通 System One 协议零转换开销，100% 保真。

## 物理验收证据 (Verification)

1. `cargo test -p ponyllm-core --lib endpoints::tests`：4 passed（包含 `test_laya_alias_canonicalizes`）。
2. `cargo test -p ponyllm-server --test systemone_tests`：5 passed（包含 `systemone_laya_routing_and_alias_test` 与 `systemone_jev_fallback_to_laya_test`）。
3. 真实网关拨测收据：
   - 请求 `http://10.42.0.185:8080/v1/systemone` 带 `model: convaiinnovations/laya`，网关成功寻路至 `vercel` 提供商、挂载密钥 `vck_6I8...` 并请求上游 `https://ai-gateway.vercel.sh/typesafe/v1/systemone`。
   - 请求 `model: laya` 成功触发别名解析与密钥池命中。
