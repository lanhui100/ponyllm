# Agent Note: zen 免费层 session id 需要 hex12 前缀（403 FreeTierError 真因）

Status: implemented

## Problem

生产网关（k8s `ponyllm`，7 pod，镜像 `sha256:56f0faeb…`）对 `opencode-zen/deepseek-v4.1-flash-free`
等免费模型 **24h 内 0 次成功 / 12 次 HTTP 403 `FreeTierError`**（"OpenCode's free tier can only be
used from within OpenCode"），而同一台机器上官方 opencode CLI、Python urllib 探针、curl（h1.1/h2）、
curl 走三条住宅出口代理、以及**同栈 rustls/hyper 探针**用同一 key、同一 payload 全部 200。

前一轮（`.agents/notes/implemented/bug-fix/2026-10-20-zen-403-fastfail-and-dsh-classifier-hardening.md`
v1.3/v1.4）把该 403 归因为「TLS 客户端指纹级拒绝」，并据此撤销出口/key 冷却。该归因**不成立**：
本次把同一 payload 分别喂给「10-14 构建的旧网关二进制」和「prod 现行二进制」，其余配置/机器/出口
完全相同——旧二进制 200、prod 二进制 403，且两者发出的请求体**逐字节相同**、请求头仅差 UA 版本与
三个可选头（逐一 A/B 均为 200）。逐项排除后定位到唯一的可变因子：**session id 取值**。

## Decision

zen 免费层对 `x-opencode-session` 的校验不是「`ses_` + 26 个字母数字」，而是
**`^ses_[0-9a-f]{12}[0-9A-Za-z]{14}$`**：前 12 位必须是小写十六进制（真实客户端放的是毫秒时钟+计数的 hex），
只有尾部 14 位是 base62。

回归来源：67e708e（2026-10-14，"align opencode zen session id encoding and update UA to 1.18.35"）
把 `format!("ses_{}", &raw[..26])`（纯 hex，天然满足前缀）改成 26 位**全 base62** 随机
（`BASE62[byte % 62]`）。前 12 位随机命中 hex 的概率约 1.5e-9，因此该提交之后网关侧 zen 免费模型
**必然 403**。本机旧二进制构建于 67e708e 之前（10-14 10:34），所以本地复现不出来——这也是
「本地探针全 200、生产全 403」长期对不上的原因。

修复（`crates/ponyllm-core/src/executor/upstream.rs`）：

1. `new_upstream_session_id()` → `ses_` + 12 位小写 hex（`((ms<<6)|seq) & 0xFFFF_FFFF_FFFF`）+ 14 位 base62；
2. `is_valid_zen_session_format()` → 严格 `^ses_[0-9a-f]{12}[0-9A-Za-z]{14}$`，下游透传值不满足该前缀形态
   时由 `clean_session_value` 丢弃并回退新生成；
3. `skills/ponyllm-zen-free/scripts/zen_free.py`：逻辑未变（`token_hex` 天然合规），仅订正 docstring；
4. 契约文档同步：`.dev-team/zen-rc/CONTRACT-v1.md` 新增 C1 v1.2 修订、`zen-free-skill-contract.md`
   与 `.dev-team/notes/zen-free-gate-probe.md` 订正为 hex12 形态。

## 物理收据

| 实验（同一 key / 同一出口 / 同一 payload / 同一分钟） | 结果 |
|---|---|
| `ses_5e67a59ef81ehxVk8zG0o0Nd9h`（12 hex + 14 base62） | 200 |
| `ses_f6236c32190eV9cvLYWlioNxSH`（12 hex + 14 base62） | 200 |
| `ses_4c09de94431dAAAAAAAAAAAAAA`（12 hex + 14×'A'） | 200 |
| `ses_GQs9Ss1p7NsR7NoY1p7hRVA5sV`（26 全 base62） | 403 FreeTierError |
| `ses_f5UiF7wZNEXb7qX7ffel4GaRb2`（26 全 base62） | 403 FreeTierError |
| `ses_PziUUHVrMVc4TPCkQnvULUGYLk`（67e708e 生成形态） | 403 FreeTierError（两次稳定复现） |
| prod 二进制（修复前）经本地抓包 relay 转发 | 403 |
| 10-14 旧二进制经同一 relay | 200（请求体逐字节相同） |
| 修复后 `cargo build --release -p ponyllm-cli` → 本地直连 opencode.ai | 200（session 实发 `ses_68574d091701HxOKciDeLTArVW`） |
| 修复后经住宅出口代理 `127.0.0.1:8898` | 200（`bunny-ok`） |

测试收据：`cargo test -p ponyllm-core` 全量 EXIT=0（lib 117 passed + 28 个集成二进制）；
新增独立验收测试 `crates/ponyllm-core/tests/zen_session_hex12_contract_tests.rs` 14 例，
在 pre-fix HEAD（2b2e521）隔离 worktree 上红相 `EXIT=101 10 passed / 4 failed`，修复后 `EXIT=0`；
生成器 1000 次抽样 `invalid=0 unique=1000`。

## Alternatives considered

1. **继续沿「TLS/HTTP 指纹」方向（复刻 BoringSSL ClientHello、换 HTTP/2 设置、改 UA 版本）**：被否——
   同一 payload 下旧/新二进制在**同一台机器、同一条出口、同一个 Python relay** 上分别得到 200/403，
   TLS 与 HTTP 层完全由 relay 承担，指纹假设被直接证伪；UA 1.18.31/1.18.35、新增 `x-opencode-project` /
   `x-opencode-request-id` / `x-opencode-session-id` 三头均单独 A/B 为 200。
2. **让下游客户端自带合法 session（透传 DSH/opencode 的 `ses_…`）**：被否——只能救「下游恰好发了合规 id」
   的调用方，网关自身生成路径仍会在无下游 session 时 100% 403；且 `x-opencode-client` 等清洗逻辑已存在，
   正确做法是让生成器本身合规。
3. **只放宽到「26 位字母数字」，不校验前 12 位**（v1.1 契约）：被否——这正是漏掉回归的原因；
   校验器必须与上游实证形态一致，否则错误的生成器能一路绿灯通过单测。
4. **session 全用 hex（`ses_` + 26 hex，旧实现）**：可过门禁但不是真实客户端形态（真实客户端尾部 14 位
   是 base62），信息熵更低且与「贴近官方指纹」的既定目标不符；采用 hex12 前缀 + base62 尾部，与实测
   200 样本同域。
5. **把 403 继续按「指纹拒绝」快返、不做任何补偿**：被否——真因是本地生成器缺陷，快返只是止血；
   若不修生成器，任何不带 session 的调用都会稳定失败。

## Consequences

- 修复前：prod 网关 zen 免费模型 100% 403（`FreeTierError`），且被误记为「指纹拒绝」导致 key/出口冷却策略
  反复调整（v1.2 → v1.3 → v1.4）。
- 修复后：网关自生成 session 通过门禁；实测直连与住宅出口代理两条路径均出现 200 出活。
- 仍需部署才生效：prod 镜像 `sha256:56f0faeb…` 含 67e708e 的坏生成器，需要重建镜像并滚动更新；
  未部署前 prod 仍会持续 403。
- 语义纠正：`.agents/notes/implemented/bug-fix/2026-10-20-zen-403-fastfail-and-dsh-classifier-hardening.md`
  中「FreeTierError = 客户端指纹级拒绝（TLS 层）」的归因作废，见该文末补记。
- 遗留（非本波写域，靠 review）：`tests/zen_session_hardening_tests.rs` / `zen_fingerprint_c2_tests.rs`
  的辅助校验器已同步收紧；`.dev-team/zen-rc/CONTRACT-v1.md` 历史段落保留原文（v1/v1.1 记录），以新增
  「v1.2 修订」小节为准。
