# Agent Note: Antigravity 配额展示收敛为仅 Gemini 系列，移除 Claude 分支

Status: implemented

## Problem

Antigravity 上游配额面已不再提供 Claude 系列额度：`retrieveUserQuotaSummary`
与 `fetchAvailableModels` 实际只返回 Gemini 系列分组与模型。前端在 Dashboard
算力池卡片（`AntigravityPoolCard.vue`）与模型管理密钥行（`KeySubSection.vue`）
仍保留完整的 Claude 提取分支、聚合水位与双进度条展示，导致三处事实性问题：

1. 上游无 Claude 分组时，Claude 水位恒为 fail-open 默认值（100%/已就绪），展示的是虚假健康；
2. 提取逻辑把"非 Claude 即 Gemini"做分区归属，一旦上游分组命名漂移，Claude
   残留数据会污染 Gemini 水位；
3. 三栏（可用性 / 5h / 周度）与双胶囊（G/C）布局为已不存在的系列预留一半版面，
   信息密度失衡。

## Decision

1. 前端只消费 Gemini 系列：`extractKeyQuota` / `extractCompactQuotas` 显式跳过
   Claude/GPT/3P 分组与 claude/gpt/sonnet/opus 模型 id（跳过而非改判归属，防止
   残留数据污染 Gemini 水位）；冷却判定、热力槽 tooltip、聚合水位只跟随 Gemini。
2. Dashboard 卡片由三栏收敛为两栏：左栏账号可用性与热力矩阵不变；右栏合并为
   "Gemini 容量"，5h 即时窗口与周度窗口上下堆叠展示。
3. 密钥行由 G/C 双胶囊收敛为单个 Gemini 胶囊（`5h | 周`），进度条加宽以填补空位。
4. 后端与 admin API 契约保持不变：`fetch_quota` 单次调用天然返回全量快照
   （无按系列单独查询的上游接口），冷却判定与 CLI `keys test` 仍需原始全量；
   后端改动只会扩大爆炸半径，不产生查询节省。

## Alternatives considered

- **方案 A：后端按系列过滤后再下发（否决）**：上游没有按系列查询接口，
  过滤只能发生在 fetch 之后，省不掉任何网络开销；且后端周耗尽冷却判定与
  CLI 全量打印仍需原始数据，过滤反而要加 `?series=` 参数污染契约，得不偿失。
- **方案 B：前端保留 Claude 展示但标注"上游未返回"（否决）**：为不存在的数据
  保留一半版面与分支复杂度，展示恒定的占位值就是误导，不如彻底删除。
- **方案 C：前端仅展示 Gemini，后端契约不动（采纳）**：删除即清理，
  API/线格式/磁盘配置零变更，单测改为"上游仍带 Claude 分组但 UI 无视"的
  回归断言，恰好锁定期望行为。

## Consequences

- Dashboard 与密钥行不再出现任何 Claude 水位；`quota-capsule-claude` /
  `claude-h5-percent` / `claude-weekly-percent` testid 删除，相关单测同步更新。
- 密钥行用量胶囊以「Gemini」全称标识（非缩写 G），5h/周百分比采用等宽数字
  （tabular-nums）与固定宽度右对齐占位，位数不足（如 5% vs 100%）时各密钥行
  用量组件仍保持上下对齐。
- 冷却/可用性语义收窄为"Gemini 周耗尽或后端 state"，Claude 周耗尽不再触发
  前端冷却误判（后端 state 冷却仍以原逻辑为准）。
- 若上游未来恢复 Claude 系列额度，需重新引入分区逻辑——届时以本记录为起点
  另起一条 feature note，不做"以后可能用得上"的保留分支。
