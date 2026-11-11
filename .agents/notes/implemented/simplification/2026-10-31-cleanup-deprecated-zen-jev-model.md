# Agent Note: 清理网关下架的 Jev 决策模型并统一至 Laya 唯一引擎

Status: implemented
Date: 2026-10-31

## Decision

1. **移除 `zen-jev` Provider 与模型配置**：
   - 从运行时配置（K8s secret `ponyllm-live-config` 与 `ponyllm-config`）及配置模板 `deploy/ponyllm-config.example.toml` 中完整移除 `[providers.zen-jev]`、`[[providers.zen-jev.model_configs]]` 与其密钥。
2. **唯一决策模型收敛至 Laya**：
   - `/v1/systemone` 决策接口仅保留 `[providers.vercel]` 下的 `convaiinnovations/laya`（支持原生名称及别名 `laya`、`laya-free`），实现 100% 干净的零样本类型化判断（System One 协议）路由。
3. **更新 Skill 文档与提示词**：
   - 更新 `skills/jev-agent-skill/SKILL.md`，清理 Jev 下架历史，明确 System One 接口直接使用 `laya` / `convaiinnovations/laya`，并通过安装脚本同步更新 user-agents 层。

## Alternatives considered

- **方案 A：继续保留 `zen-jev` 配合 fallback 降级**：
  - 缺点：每次请求均需先尝试 `zen-jev` 并遭遇 401/451 失败，增加不必要的网络开销与延迟，并污染审计日志。
  - 结论：否决。上游已彻底下架且无恢复计划，应果断清理。
- **方案 B：将 `jev-1.13-free` 设为 `convaiinnovations/laya` 的别名**：
  - 缺点：掩盖了模型下架客观事实，误导上层调用方以为旧模型依然存在。
  - 结论：否决。契约层面应当明确引导客户端使用真实模型 `laya`。

## Reintroduction conditions

当且仅当 OpenCode Zen 上游发布了替代原 Jev 模型且明确标识支持 `jev-1.13-free` 的全新端点，
且在该端点上通过了至少一个生产级完整性回归周期（满 7 天零 Transport/401 失败），
才考虑将其配置作为 `laya` 之后的备选 Fallback，且必须声明新的 Provider entry 以区分缓存。

## Consequences

- 网关配置减少了一个废弃的 Provider（`zen-jev`），消除无效的出海请求与重试开销。
- `/v1/systemone` 接口直接路由至 Vercel AI Gateway 承载的 Laya，平均响应时延降低并保持稳定。
- 所有使用 System One 决策的 Agent 规范化传参为 `model: "laya"`。