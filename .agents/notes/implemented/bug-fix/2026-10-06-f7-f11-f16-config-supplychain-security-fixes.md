# Agent Note: F7-F11+F16 配置与供应链安全修复实施（Phase 2 红队整改）

Status: implemented

## Problem

Phase 2 红队修复（FIX-CONTRACT.md 批 3 配置/供应链并行）需落地以下安全项：
- **F7（VULN-04）** rustls 0.23.43 命中 RUSTSEC-2026-0285（TLS1.3 握手加密层级混淆，CVSS 5.3），需 ≥0.23.45；
- **F8（VULN-09）** CI 无 cargo/pnpm 依赖审计门禁，web 依赖树 brace-expansion 2.1.4 存在 3 漏洞（2 high 1 moderate）；
- **F9（VULN-13）** install.sh/install.ps1 无 sha256 校验、release 无 .sha256 资产（curl|bash / irm|iex 分发供应链）；
- **F10（VULN-10 A 部分）** 缺 CORP/COOP、permissions-policy 缺 payment、CSP 无 upgrade-insecure-requests；
- **F11（VULN-20）** 静态哈希资产无显式 Cache-Control；
- **F16（VULN-01 边缘部分）** admin 专用限流中间件 `ponyllm-admin-ratelimit`（20/10）定义后从未被 IngressRoute 引用（admin 规则漏挂）。

## Decision

按 config-review.md 方案实施（全部 A 类，本仓独立可落地）：

1. **F7**：`Cargo.toml` rustls 声明下限 `"0.23"` → `"0.23.45"`；`cargo update -p rustls` 解析 `Cargo.lock` 0.23.43 → 0.23.45（tokio-rustls 0.26.4 / postgres_rustls 0.1.4 / kube 0.95 / reqwest 0.12.28 的 `^0.23` 约束均兼容，无需连带升级）。`cargo audit` 漏洞数归零（unmaintained/unsound 5 项警告为 fxhash/paste/rustls-pemfile/lru，间接依赖，记录暂缓）。
2. **F8**：`web/package.json` 增 `pnpm.overrides { "brace-expansion": "2.1.7" }` 并刷新 `pnpm-lock.yaml`（2.1.4→2.1.7）；`.github/workflows/ci.yml` 新增独立 `audit` job（`taiki-e/install-action@v2` 装 cargo-audit + `cargo audit`；`pnpm audit --dir web --audit-level high`），并入 `build-and-push-image` 的 `needs`（带漏洞不发布）。
3. **F9**：`install.sh` 下载后校验 `${DOWNLOAD_URL}.sha256`（sha256sum -c，macOS 兜底 shasum -a 256 -c，双失败即中止）；`install.ps1` 用 `Get-FileHash` 比对（不一致 throw）；`release.yml` 新增 "Compute checksum" 步骤并随 Upload 附 `.sha256` 资产（windows bash 有 sha256sum、macOS 走 shasum 分支）。
4. **F10**：`deploy/ponyllm-ingress-hardening.yaml` customResponseHeaders 增 `Cross-Origin-Resource-Policy: same-origin`、`Cross-Origin-Opener-Policy: same-origin`、`Permissions-Policy` 补 `payment=()`（对齐 app.rs 应用层默认）、CSP 增 `upgrade-insecure-requests`。HSTS preload 与 COEP 属 B/C 类，不在本期。
5. **F11**：`crates/ponyllm-server/src/app.rs` build_web_router：`/assets` ServeDir 包 `assets_cache_headers` 中间件（成功响应加 `public, max-age=31536000, immutable`）；index 路由（`/`、`/connect`、`/dashboard`、`/recorder`、`/governance`）包 `html_no_cache`（`no-cache`，保证发版后引用新哈希）。实现复用 favicon 的 from_fn 模式，零新增依赖（tower 已存在，加 `use tower::ServiceExt;`）。
6. **F16**：`deploy/ponyllm-ingress-routes.yaml` 规则 #5（`/api/admin/*`）middlewares 链补挂 `ponyllm-admin-ratelimit`（置于全局 ratelimit 之前，链式叠加更严的 20/10 限流）。

## Alternatives considered

1. **cargo audit 放 test job 而非独立 job**：test job 是 3-OS matrix，audit 会重复跑 3 次浪费构建时间；独立 `audit` job 与 test/web 并行、一次跑完且可独立阻塞镜像发布。采纳独立 job。
2. **brace-expansion 修复用升级 vue-tsc 而非 pnpm overrides**：vue-tsc 2.2.0 的 @vue/language-core 锁 minimatch 9.0.9→brace-expansion 2.1.4，升级 vue-tsc 是"顺带修复"但会引入更大升级面与回归风险；overrides 精确固定 2.1.7（npm 2.x 线最新修复版）改动最小且 lockfile 可复现。采纳 overrides；vue-tsc 升级列为长期项。
3. **F11 用 tower-http `set-header` feature 而非手写 from_fn**：需在 Cargo.toml 开启 `features=["set-header"]`，扩大 feature 面；favicon 已有同款 from_fn 先例，零依赖改动。采纳 from_fn。
4. **install 校验和放二进制级（解压后再验 exe）**：.sha256 按 artifact（tar.gz/zip）生成更简单且与 release 资产一一对应，解压内二进制校验需发布二进制级校验和（额外资产）；本期按 artifact 级校验，README 注明信任边界。
5. **F16 依赖后端限流（F2）不挂边缘中间件**：F2 是 per-pod 内存限流、F4 是后端 IP 围栏，边缘 admin-ratelimit 是独立纵深层且当前 0 成本可挂；挂载链式叠加，不冲突。

## Consequences

- 验收：`bash scripts/sec-acceptance.sh` 14 项全 PASS；`cargo audit` 0 漏洞；`pnpm audit` 0 漏洞；rustls 0.23.45 全链编译通过；`app.rs` 改动无编译错误。
- 当前全量 `cargo test --workspace` 与 `pnpm build` 受批 1/批 2 并发中间态阻塞（state.rs AuthRateLimiter Debug 缺 derive=F2；wizard.rs GatewaySection 新字段未同步=F1/F4；markdown.test.ts 引用未实现模块=F12），非本批改动回归，待各批完成后统一复验。
- B/C 类后续：HSTS preload（EdgeOne 控制台）、限流 depth:2（cluster-infra trustedIPs）、COEP/SRI/cosign（二期评估）。
- 提交纪律：本次仅 git add 本批文件（Cargo.toml/lock、web package.json/lock、ci.yml、release.yml、install.sh/ps1、ingress-hardening.yaml、ingress-routes.yaml、app.rs、本 ADR），严禁 add ./-A（工作区含其他批次的并发修改）。
