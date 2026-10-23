# 迁移指引：socks5 / socks5h 出口被拒绝（contract v3 / v6）

- 生效版本：Wave-2 起 `validate_egress_entry` 拒绝 socks；Wave-2b 起 admin 写路径
  `check_proxy_url_fast` 同口径拒绝（provider / model 的 `proxy` 字段也覆盖）
- 日期：2026-10-23
- 相关契约：`.dev-team/contracts/2026-10-07-egress-pool-contract.md`、
  `crates/ponyllm-protocol/tests/security_extra_sanitize_acceptance.rs`（v4）、
  `crates/ponyllm-server/tests/security_egress_v6_acceptance.rs`（v6）

## 1. 症状与生效位置（务必按实际行为理解）

**校验点只有一个显式失败面 + 一个静默丢弃面：**

| 入口 | 行为 |
|------|------|
| admin 写路径（PUT/POST provider、model 的 `proxy` 字段；egress_pool 条目写入） | ❌ **立即拒绝**，HTTP 400，文案含 `socks` |
| `egress_pool` 里的 socks 条目（直接编辑 TOML / K8s Secret） | ⚠️ **配置加载不报错**；网关构建 egress pool 时逐条校验，**warn + 丢弃该条目**（日志 `dropping invalid egress pool entry`）；若 pool 被丢空，则回退到"该 provider 无 pool"的语义（其 `proxy` 字段 / 默认直连） |
| provider/model 的 `proxy` 字段（直接编辑 TOML） | ⚠️ **加载期不校验**；运行期该代理无法构建，回退到显式 no-proxy 直连并 warn —— 出网会静默变成直连 |

admin 写路径的错误样例（凭据已由 `ponyllm_core::sanitize_proxy_url` 脱敏为 `***`，http(s) 条目序列化会带尾部 `/`）：

```
proxy URL 'socks5://***:***@127.0.0.1:1080' uses the unsupported 'socks5' scheme:
socks proxies are not built into this gateway (reqwest without the socks feature),
so the proxy would silently dial direct — use an http(s) CONNECT proxy instead
```

egress_pool 条目被拒时的文案（出现在 400 响应；同样脱敏）：

```
egress entry 'socks5://***:***@127.0.0.1:1080' uses the unsupported 'socks5' scheme: ...
```

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
   # 启动网关（注意：子命令是 serve，ponyllm run 不存在）
   ponyllm serve --config ponyllm.toml --no-web
   ```

   配置加载本身**不会**因 socks 条目失败（见 §1）；真正的显式拦截面是 admin 写接口的 400。

## 3.1 如何自查是否残留 socks 条目

```bash
# 1) 静态自查：配置文件 / 部署清单里是否还有 socks 字样
grep -rniE "socks5h?://" ponyllm.toml deploy/*.yaml deploy/*.toml 2>/dev/null

# 2) 运行期自查：egress_pool 里的 socks 条目会被 warn+丢弃，日志关键字如下
#    （若出现，说明该出口没有生效，请求实际走了 pool 内其它出口或直连）
ponyllm serve --config ponyllm.toml --no-web 2>&1 | grep -i "dropping invalid egress pool entry"

# 3) admin 写接口自查：对目标 provider 重新 PUT 一次
#    返回 400 且文案含 "socks"  → 该条目仍非法，必须迁移
#    返回 200                    → 该条目已合法
```

判定口径：**日志里没有 `dropping invalid egress pool entry`、且 provider PUT 为 200**，才算迁移干净。

## 3.2 回滚（若迁移后出网被证实不可用）

**回滚前提**：先确认 socks 出口确实是该 provider 的**唯一**出网路径（其它 http(s) 出口/直连不可达或不被上游接受）。
若只是某个出口不可用，优先在池内增加/替换 http(s) 出口，而不是回滚策略。

**回滚步骤**（回滚的是"出口形态"，不是"是否校验"）：

1. 把 `egress_pool` 中的 socks 条目替换为等价的 **`http(s)://` 代理**（pproxy 的 http 口、
   SOCKS→HTTP 桥接暴露的本地端口等）；若该 provider 本应直连，则把 `proxy` **置空**。
2. 重启网关：`ponyllm serve --config ponyllm.toml --no-web`。
3. 用 §3.1 的三条命令复核（grep 静态扫 / `dropping invalid egress pool entry` 日志扫 / provider PUT 200）。
4. 验证真实出网 IP 与预期一致（例如经该出口访问回显 IP 的服务），确认没有静默直连。

**明确**：回滚到**旧版本二进制不会恢复 socks 支持**。旧版本同样不会真正经 socks 出网——
它的行为是"请求期代理构建失败 → 回退直连并 warn"（provider `proxy`）或"启动时 warn + 丢弃该条目"
（`egress_pool`），即静默脱钩仍然存在。降级无法解决该问题，只能改成 `http(s)` 或直连。

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
