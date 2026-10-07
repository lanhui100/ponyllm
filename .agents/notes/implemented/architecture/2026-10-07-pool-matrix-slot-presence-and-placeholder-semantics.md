# ADR: 算力池矩阵方块占位颜色与存在性语义规范

Status: implemented
- 日期: 2026-10-07
- 决策人: Dev Team

## 背景 (Context)

在 PonyLLM Web Dashboard 的算力池卡片（`AntigravityPoolCard.vue`）中，核心视觉呈现之一是账户可用性状态的热力矩阵（Matrix Heatmap Grid）。
该矩阵包含两种方块：
1. **已分配账号槽位 (`slot-heatmap-cell`)**：代表当前网关中实际配置并接入的 Antigravity 账号；
2. **未占用预留槽位 (`slot-heatmap-empty`)**：代表当前矩阵容量未使用的预留空白槽位，提示“未配置槽位 · 接入新账号后将自动点亮”。

此前实现中，当一个账号**存在**但尚未获取到新鲜配额（如未探测、缓存过期或仅返回了非 5h 配额而导致 `g5hFraction == null` 时），其热力色块被硬编码赋为了 `bg-[#d0d7de]`。与此同时，未占用预留槽位也是 `bg-[#d0d7de]`。
这导致用户在视觉上无法区分：
- 究竟是**账号存在，仅由于探针未跑完/无配额信息而等待刷新**？
- 还是**根本没有接入账号，仅为空占位槽位**？

这二者具有明确的业务与运维语义差异。

## 决策 (Decision)

1. **已配置账号未查到额度的占位色标**：
   - 当账号存在于池中，但在配额探测链路中未查到配额信息（`!testResult || !isQuotaResultFresh || g5hFraction == null`）且未处于冷却/硬故障态时，热力色块必须使用**极浅绿色**占位，与 GitHub 单色阶底色保持视觉同系与层级区分。
   - 选用极浅绿代号：`bg-[#e6f4ea]`（或带边/对比明显的清新极浅绿色 `bg-[#ebfbee]` / `bg-[#e6f4ea]`，并保持 hover 态轻微深绿如 `hover:bg-[#ceead6]`），明确传达“该账号已存在且可用/等待配额”，而非空槽位。
   - 对应 `slot-heatmap-cell` 的 class 判定分支更新为此极浅绿色体系。
2. **未配置空槽位的占位色标**：
   - 保持中性灰阶实体方块 `bg-[#d0d7de]`（hover: `bg-[#afb8c1]`），维持其“无账号/空槽位”语义不变。
3. **配额刷新排查结论**：
   - 针对“当前有多个账户刷新配额查不到信息”的问题，排查确认：
     a) 网关后端对 Antigravity 账号支持两级查询：`/v1internal:fetchAvailableModels`（模型级）与 `/v1internal:retrieveUserQuotaSummary`（5h/周度窗口级）；
     b) 当账户存在资格受限（如 `RESTRICTED_AGE` 403 错误，见 `ag-caysonsiddall@gmail.com`）时，上游不返回任何 quota groups；
     c) 此外，前端在页面首屏及卡片中，对未 probe 过的 key 仅在用户手动点击“刷新配额”或后台 `handleRefreshMissingQuotas` 串行执行完毕后才会落盘 localStorage；若前端缓存过期或单次请求锁竞争（`lock_busy`），方块即处于“等待刷新配额”状态。
     d) 修正矩阵方块色阶后，即使该账号等待刷新，方块也会清晰呈现极浅绿色，消除“看起来像没有配置账号”的混淆。

## 替代方案 (Alternatives considered)

1. **使用纯白或带边框透明色 (`bg-transparent border border-slate-300`)**：
   - 缺点：与矩阵整体的实体方块风格不统一，视觉重心脱节。
2. **直接判定为 0% 额度（耗尽浅绿/薄荷绿）**：
   - 缺点：把“未知（未查到）”误判为“已查到且额度为0”，误导运维人员以为额度已打满或耗尽。
3. **保持灰阶，仅在 tooltip 中区分**：
   - 缺点：违反首屏直觉感知原则，无法一眼识别账号容量部署规模。

## 影响 (Consequences)

- 强化了算力池矩阵的视觉语义精度：极浅绿 = 存在账号等待配额；灰色 = 空闲插槽无账号。
- 所有单元测试必须对“未探测/等待刷新”的 cell class 断言由 `bg-[#d0d7de]` 调整为极浅绿 `bg-[#e6f4ea]`。
