# NFR 达标报告: wave-6 用户面只读模型清单与新增 Token「允许模型」多选下拉

- **版本/波次**: wave-6 (user-models-picker)
- **状态**: 达标 (PASS)
- **基准比对源**: `.dev-team/nfr-baseline-wave-6.json`
- **契约定义**: `.dev-team/contracts/wave-6-user-models-picker.md`

## 逐项比对与客观证据

| 检查项 | 基准要求 | 本次实现与实测证据 | 结论 |
|---|---|---|---|
| **外部调用超时** | `external_call_timeout_ms <= 3000` | 后端 `GET /api/user/models` 仅只读进程内状态（`list_all_models` + `check_access`），零外部网络/DB/RPC；前端仅单次同源 fetch，基准声明 3000ms 合规。 | **PASS** |
| **重试与快速失败** | 不引入无限重试，不影响既有退避 | 后端端点无失败重试面；前端失败展示静态重试提示（`token-models-retry`），仅由用户显式点击触发单次重新加载，不自动轮询风暴；列表失败不阻断 token 创建。 | **PASS** |
| **并发与锁安全** | `concurrency_lock.frontend_concurrency`（单飞 + 代次守卫） | 前端落地 `modelsInFlight` 请求单飞复用 + `modelsReqSeq` 请求代次守卫（陈旧响应静默丢弃），页面级缓存；4/4 竞态回归测试通过；后端 50 路并发对抗测试全部 200 且字节级一致。 | **PASS** |
| **日志与防吞异常** | `logging.format=json`，无裸 pass | 错误未被吞，401 统一由中间件/unauthorized 处理；前端非 2xx 抛出携带 code/status 的 Error；未向控制台打印无上下文错误。 | **PASS** |
| **资源释放** | 无句柄/连接泄漏 | 前端下拉面板在关闭与 `onBeforeUnmount` 时严格解绑 `mousedown`/`keydown` 监听器；fetch 响应体消费即释放；无常驻定时器。 | **PASS** |
| **配置与自检** | 无新增配置项 | 无新增配置项或环境变量，开箱即用，与既有用户面鉴权架构完全正交。 | **PASS** |

## 机器验收物理收据

1. **Rust 契约与对抗测试 (`cargo test -p ponyllm-server --test red_user_models_api_tests --test adversarial_user_models_tests`)**:
   - `red_user_models_api_tests`: `test result: ok. 8 passed; 0 failed; 0 ignored; finished in 3.86s` (Exit Code 0)
   - `adversarial_user_models_tests`: `test result: ok. 1 passed; 0 failed; 0 ignored; finished in 1.21s` (Exit Code 0)
2. **Web 契约、对抗与竞态回归测试 (`cd web && pnpm vitest run src/views/TokensView.models-race.test.ts src/views/TokensView.test.ts src/lib/userApi.models.test.ts src/lib/userApi.models.adversarial.test.ts`)**:
   - `Test Files: 4 passed (4), Tests: 34 passed (34)` (Exit Code 0)
3. **Web 全量回归与类型检查**:
   - `cd web && pnpm test`: `Test Files 37 passed (37) / Tests 238 passed (238)` (Exit Code 0)
   - `cd web && pnpm typecheck`: Exit Code 0
   - `cd web && pnpm lint`: 0 warnings, 0 errors, Exit Code 0
4. **真实浏览器端到端 E2E (agent-browser)**:
   - 全链路真实表单登录、弹窗展开、9 项模型全部可命中（0 clipped）、真实鼠标点选 2 项、提交创建、结果弹窗展示明文 Token、控制台错误 0。
   - 截图收据归档：`docs/release/wave-6-token-models-picker.png` 及 `docs/release/wave-6-token-created.png`
