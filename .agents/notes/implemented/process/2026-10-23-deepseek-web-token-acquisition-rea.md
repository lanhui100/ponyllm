# Agent Note: deepseek-web token 获取机制（短信登录）REA 与人机验证边界

Status: implemented

## Problem

deepseek-web provider 的凭据是 chat.deepseek.com 网页登录态 token，此前只能在浏览器手工复制，失效（40003）后必须再次人工登录；bundle 中 cookie 不参与 API 鉴权、无 refresh 端点。backlog 要求以 REA 明确 token 的获取机制，并判断是否存在「手机验证码登录」的合规自动化路径（不触碰验证码/WAF 绕过边界）。

## Decision

以生产 bundle（`/tmp/ds-main.js`，appVersion 2.5.0 / commitId 6c40ca01）静态取证 + 三次无副作用活体探针，冻结以下事实与边界：

1. **短信登录路径的 token 签发点**：`POST /api/v0/users/login_by_mobile_sms`，成功 `biz_code=0|1`，token 在 `data.biz_data.user.token`；密码登录/注册接口同样返回 token，但需密码 + MFA，本账号路径不依赖。
2. **登录请求的两个硬前置**：请求体加密（AES-CBC 随机 key/IV + RSA-OAEP/SHA-256 封装，头 `x-request-encryption-key`）与 guest PoW（`POST /api/v0/users/create_guest_challenge` → 23 轮 Keccak 求解 → 头 `X-DS-Guest-PoW-Response`）。缺 PoW 实测 `40300`。
3. **短信发送强制人机验证**：`POST /api/v0/users/create_sms_verification_code`（`scenario=login`）在无 captcha token 时实测 `biz_code=2 RECAPTCHA_VERIFY_FAILED`，且发生在手机号校验之前；大陆数美、海外 Turnstile/hCaptcha。
4. **合规自动化上限 = 人类在环**：不实现验证码/WAF/指纹绕过。token 获取与续期（无 refresh 端点，续期即重新登录）由人工在浏览器完成验证码与短信码；`scripts/deepseek_web_login.py` 实现“人工短信码 → 加密登录 + guest PoW → token”，并新增 `--status`/`--verify` 禁言前置门（`users/current` 的 `chat.is_muted`，exit 14）；不做全自动短信发送。
5. **可复现凭证**：新增只读探针 `scripts/deepseek-web-token-rea-probe.py`（dummy 非法手机号，不发短信；三条断言全过才 exit 0），机制证据落 `.dev-team/contracts/wave-13-deepseek-web-token-rea-evidence.md`；运行手册 `docs/deepseek-web-provider.md` §4/§5.1 更新。

## Alternatives considered

- **全自动短信登录（脚本自行请求验证码 → 登录）**：否决。服务端强制 captcha（实测 2），自动求解数美/Turnstile/hCaptcha 属硬性合规禁区；且触发风控（`11 RISK_DEVICE_DETECTED`）风险高。
- **HTTP 半自动（人工在浏览器发短信，CLI 用同一号码换 token）**：暂缓。合规但未验证设备绑定：短信请求与登录请求 `device_id` 不一致时是否返回 `11`/是否接受跨设备短信码未知，属 `[UNVERIFIED]`，不作为默认路径。
- **密码登录替代短信**：不可作为通用路径。`/api/v0/users/login` 支持密码 + MFA（`OTP_REQUIRED`/`SIGNATURE_REQUIRED`），但账号未必设置密码，且同样受风控/设备签名约束，收益低于人工在环取 token。
- **浏览器辅助脚本（Playwright 打开 `/sign_in`，人工完成验证码与短信码后读 token）**：本轮不实现，仅登记为可选项。人工步骤不变，只是把「DevTools 复制」换成脚本读取，收益有限；待明确需求再做。

## Consequences

- 运行手册与 backlog 不再把 token 获取列为未知研究项；运维按「浏览器人工登录 → 读 `localStorage["userToken"]` → Secret 轮换」执行。
- 探针是活体网络检查（依赖 chat.deepseek.com 可达、bundle 版本漂移可能改变断言），不进入 CI；失败时非零退出并显式报错。
- 验证码门禁是产品级硬约束：即使网关侧想自动化，也必须保持「显式失败 + 人工处理」，与 wave-9 契约 DW-G-* 一致。
- 2026-10-23 真实登录已验证（`biz_code=1`、token 签发、`create_chat_session` 成功），但该账号随即被上游禁言（`biz_code=5`，`mute_until=2026-10-26 07:30:42`），chat 链路不可用；禁言归因与流量隔离见 ADR `2026-10-23-deepseek-web-mute-risk`。
- 若上游未来提供官方 OAuth/API token 或长期凭据，本决策应重开（现无此端点：bundle 仅有第三方 `/oauth/get_token`）。
