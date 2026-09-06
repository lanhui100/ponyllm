# WEB-01 脚手架与Shell

## Basic Info
- ID: WEB-01
- Status: Ready
- Priority: P0
- Owner: 待定
- Created At: 2026-09-06
- Updated At: 2026-09-06
- Estimated Effort: 0.5周
- Blocker: 无
- Unblock Condition: 无

## Goal
建 `web/` 跑通路由壳、纸感主题、connect 守卫与双门禁。

## Output
- `web/src/router.ts`
- `web/src/lib/alova.ts`
- `web/oxlint.config.json`

## Acceptance Criteria
1. `pnpm --dir web lint` 绿（oxlint --deny-warnings）。
2. `pnpm --dir web typecheck` 绿（vue-tsc）。
3. `/connect` 401 踢回可用 review 演示。

## Current Progress
- 骨架未建，待开工。

## Next Action
- 建 vite8 工程并跑 `pnpm --dir web lint`。

## Resume Hint
- 直接打开本卡跑 Next Action；需 rationale 见 Related Files ADR。

## Review Summary
- 待审核。

## Related Files
- ADR: `.agents/notes/proposed/architecture/2026-09-06-web-console-ia-and-stack.md`
- ADR: `.agents/notes/proposed/process/2026-09-06-web-toolchain-quality.md`
