# 替代治理形态适配任务书

> 由 `ponygo init` 生成：**幂等重写，勿手工编辑**（检测清单真源 = `ponygo` 的 `detect_alt_gov_hits`）。
> 本文件**不阻断**装配（`ponygo init` 照常完成，rc=0）；它替代「先别注入、先去 audit」的旧阻断叙事。
> 未登记适配决策前 `ponygo status` / `ponygo audit` 会持续 WARN；全局完成判据 = 落一条
> `.agents/notes/**/process/*alt-governance*.md` 写明下表各项的处置。

## 检出项（1 项）

| # | 检出项 | 类别 |
|---|---|---|
| 1 | `.github/workflows` | 远端流水线 |

## 逐项适配（检出项 → 语义映射 → 唯一动作 → 完成判据）

### 1. `.github/workflows`（远端流水线）

- **检出项**：`.github/workflows`（远端流水线）
- **语义映射**：与 `.meta/gates/ci-verify.sh`（A4 远端门）同权——流水线只做「调一步门」的薄壳，不承载判定逻辑
- **唯一动作**：在既有流水线加一步 `bash .meta/gates/ci-verify.sh`，不要新建 workflow
- **完成判据**：流水线里出现 `bash .meta/gates/ci-verify.sh` 这一步，且一次 PR 运行通过
