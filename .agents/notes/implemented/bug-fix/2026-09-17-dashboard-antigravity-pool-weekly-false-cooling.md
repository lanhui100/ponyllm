# Agent Note: Dashboard 算力池缺 quota_groups 时周水位误判冷却

Status: implemented

## Problem

Dashboard 的 Antigravity 算力池点击“刷新配额”后数据看似毫无更新，
而模型管理页同一账号的刷新展示正常。实证（2026-09-17，UTC）：

- 后端拨测接口全部 `200`（`probe ok (quota fetched for 33 models)`），
  单次 7–25s；`quota_groups` 间歇缺失（一次 `NONE`，一次正常），
  原因是 `retrieveUserQuotaSummary` 只有 4s 超时，冷建连下会回退为
  仅 `fetchAvailableModels` 平铺列表。
- 平铺列表的 33 个模型无任何周窗口标识（`remaining_fraction` 只有
  `0.0068 × 26` 与 `1.0 × 7` 两档），`weeklyFraction` 在前端恒为初始值 0。
- `AntigravityPoolCard` 把 `weeklyFraction <= 0` 直接判为冷却并剔出聚合，
  导致后端已 `active` 的账号在 Dashboard 仍显示冷却、聚合水位锁死 0%。
  模型管理页对缺失周数据用 `?? 1.0` 回退，所以显示正常——两页分叉点在此，
  与 pproxy 最近提交无关（Cloud Code 三 host 有合规出口例外，仍走 Vercel；
  拨测经由同一代理全部成功）。

## Decision

`web/src/components/AntigravityPoolCard.vue` 的 `extractKeyQuota` 现在显式区分
两种数据源语义：

- `quota_groups` 存在时按 `weekly`/`5h` 桶写入，并记录每系列周桶是否真实出现；
  周识别口径与模型管理页 `extractCompactQuotas` 完全对齐（含
  `description`/`display_name` 的 week/周/7d 变体）；
  未出现的周窗口默认 `1.0 / 已就绪`（未知≠耗尽），与模型管理页 `?? 1.0` 对齐。
- 仅有平铺 `quota`（`fetchAvailableModels`，只表达 5h 滚动余量）时，
  按同系列最小值聚合为 5h 水位（此前是遍历覆盖、末值胜出），
  周水位一律记 `1.0 / 已就绪`，冷却判定交还后端 `state`。
- `isKeyCoolingDown` / 热力矩阵 / 聚合均值逻辑不动：后端 `cooling_down`
  仍强制冷却，真实周 0 仍判冷却；只有“未知周”不再误判。

配套在 `AntigravityPoolCard.test.ts` 新增防回归用例：
后端 `active` + 仅平铺配额（无 `quota_groups`）时显示 `1/1 账号就绪`，
不出现“所有账号冷却中”；平铺 gemini 用 0.0068/0.5 不同值证伪最小值聚合
（末值胜出会得 50% 而非 1%）；四个水位用 `data-testid` 精确断言。
已知取舍：`remaining_fraction: null` 按 `?? 0` 计入最小值（与模型管理页同式，
无信号按耗尽处理）；平铺周判定的 `includes('d')` 单字母过宽问题与模型管理页
同源，本次保持两页一致，未单独收紧。

## Alternatives considered

- A（采用）：未知周默认健康（1.0），冷却以后端 `state` + 真实周 0 为准。
  改动面最小（单文件函数 + 单测），与模型管理页语义一致；风险是周接口长期
  失败时周水位显示 100%，但后端冷却徽标与倒计时仍真实，误导面可控。
- B（落选）：Dashboard 刷新改为串行/错峰探测，根治 `quota_groups` 间歇缺失。
  实测并行 5 路全部 200 成功，缺失只影响单次摘要超时；串行把刷新拉长到
  2 分钟级，体验代价大而根因（聚合误判）仍在，不治本。
- C（落选）：后端拨测在 `quota_groups` 缺失时不清冷却。
  会让账号长期卡在冷却直到某次摘要成功，反而加重“刷新无更新”的观感；
  且平铺模型仍有正余量时解冻是合理行为，保持不动。

## Consequences

- Dashboard 与模型管理页在缺摘要时展示一致：5h 显示真实低水位，
  周显示 100%（未受限假设），冷却只跟随真实状态。
- 验证：`pnpm --dir web test --run`（vitest 全绿）；
  `pnpm --dir web exec vue-tsc --noEmit` 通过后重建 `web/dist`
  （gitignored，网关静态托管，改后刷新页面即生效，无需重启网关）。
