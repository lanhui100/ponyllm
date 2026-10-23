# 迁移指引：socks5 / socks5h 出口被拒绝（contract v3 / v6）

- 生效版本：Wave-2 起 `validate_egress_entry` 拒绝 socks；Wave-2b 起 admin 写路径
  `check_proxy_url_fast` 同口径拒绝（provider / model 的 `proxy` 字段也覆盖）
- 日期：2026-10-23
- 相关契约：`.dev-team/contracts/2026-10-07-egress-pool-contract.md`、
  `crates/ponyllm-protocol/tests/security_extra_sanitize_acceptance.rs`（v4）、
  `crates/ponyllm-server/tests/security_egress_v6_acceptance.rs`（v6）

## 1. 症状

写入或启动校验时报错（HTTP 400 / 配置加载失败），错误文案含 `socks`：

```
egress entry 'socks5://user:***@127.0.0.1:1080' uses the unsupported 'socks5' scheme:
socks proxies are not built into this gateway (reqwest without the socks feature),
so the entry would silently dial direct — use an http(s) CONNECT proxy instead
```

admin 写路径（provider / model 的 `proxy`）：

```
proxy URL 'socks5://user:***@127.0.0.1:1080' uses the unsupported 'socks5' scheme: ...
```

错误文案里的 `user:pass` 已脱敏（`ponyllm_core::sanitize_proxy_url`），不会回显口令。

## 2. 根因

本 workspace 的 `reqwest` **未编译 `socks` feature**：`Proxy::all("socks5://…")`
即使被接受也无法真正经由该代理出网，运行时会回退到**直连**——出网语义静默脱钩：
- 期望经 pproxy / 海外代理出海的请求实际从网关本机 IP 发出（地域封锁、出口 IP 记账、
  额度桶分裂都会失真）；
- SSRF 姿态被削弱：出网校验与真实拨号路径不一致。

因此校验层直接 fail-closed，而不是"接受但静默降级"。

## 3. 迁移步骤

1. 找出所有 socks 条目：

   ```bash
   grep -rn "socks5" ponyllm.toml deploy/*.toml
   ```

2. 换成 **http(s) CONNECT 代理**（pproxy 的 http 端口、企业正向代理等都是这种形态）：

   ```toml
   # 之前（被拒绝）
   # proxy = "socks5://127.0.0.1:1080"
   # egress_pool = ["direct", "socks5://egress-1.example.com:1080"]

   # 之后（合法）
   proxy = "http://127.0.0.1:8899"
   egress_pool = ["direct", "http://egress-1.example.com:8899"]
   ```

3. 若上游代理只提供 socks 口，用一个本地 SOCKS→HTTP 桥接（pproxy / gost 等）把
   `http://127.0.0.1:<port>` 暴露给网关；网关侧只配置 http(s) 条目。

4. 重新校验：

   ```bash
   # 配置加载（启动即校验 egress_pool / proxy）
   ponyllm run --config ponyllm.toml --no-web
   # admin 写路径（PUT provider）返回 400 且文案含 socks = 期望行为
   ```

## 4. 仍被接受 / 被拒绝的条目（速查）

| 条目 | 结果 |
|------|------|
| `direct` / `none` / 空 | ✅ 直连（网关本机出口） |
| `http://127.0.0.1:8899`、`http://localhost:7890` | ✅ 本地 pproxy（含 `[::ffff:127.0.0.1]` 等 loopback 内嵌形态） |
| `https://egress.example.com:443`、`http://user:pass@egress.example.com:8899` | ✅ 公网/凭据代理 |
| `socks5://…` / `socks5h://…`（任意大小写） | ❌ 拒绝（本指引） |
| `http://10.0.0.5:8080`、`http://169.254.169.254:80`、`http://[::ffff:10.0.0.1]` | ❌ 私有/元数据目标 |
| 6to4 / Teredo / ISATAP / NAT64(`64:ff9b::/96`,`64:ff9b:1::/48`) / SIIT(`::ffff:0:0/96`) 内嵌私有 v4 | ❌ 内嵌 IPv4 判定为私有/元数据 |

## 5. 已知残余风险（不在本次范围）

- 代理**主机名**在配置期不做 DNS 解析（`nip.io` 之类把私有 IP 编码进域名的形态仍可能绕过）；
- CLI `dialtest` 路径未接入 egress 校验；
- config 与 server 各有一份 IPv6 内嵌地址判定 helper（行为已对齐，未去重）。
