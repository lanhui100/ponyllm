# P1 实施产物对抗安全审核报告（红队 / sec-reviewer）

- 审核对象：Phase 1 实施产物，5 commits `678629b..c92ebf8`（基线 d16ac5c，diff 限 crates/ 与 scripts/）
- 审核日期：2026-09-28
- 审核方式：只读代码审计 + 本机只读测试验证（`cargo test` 定向跑安全相关测试全绿，未触碰生产集群、无写操作）
- 审核范围：① SA/凭据面（kube-rs in-cluster 配置、TLS 链、patch 盲写）；② 冲突映射信息泄漏；③ PG advisory lock 闭环；④ 优雅停机 drain 丢写；⑤ 刷新写回闭环；⑥ kube/k8s-openapi 依赖面

## 测试验证记录（只读，全部通过）

| 测试 | 结果 |
|---|---|
| `admin_store::tests`（8：CAS 轮转/冲突/缺 key/base64 线格式） | 8 passed |
| `refresh_lock::tests`（2：双副本串行化/指标计数） | 2 passed |
| `config_poller::tests`（4：三态/停机/错误不 panic/hash 稳定） | 4 passed |
| `kubernetes_store_wiremock_tests`（5：patch body 携带 rv / 409→Conflict / 轮转 / 404 / 500） | 5 passed |
| `antigravity::ha_gate_tests`（2：skip 不触发 persist / acquire 时 persist 在锁内） | 2 passed |
| `graceful_shutdown_tests`（2：SSE 自然排空 / 超限强断） | 2 passed |
| `admin_contract_tests` + `admin_write_tests`（适配后，13） | 13 passed |

---

## 总体结论：**通过（有条件通过）**

Phase 1 代码本体**未发现 S1 阻断项**；T0 报告的两项 S1（CNI flannel 不执行 NetworkPolicy、Secret 静态加密未启用）不在本 diff 范围，属于 Phase 0/2 集群前置，仍待完成（沿用 d16ac5c 裁决，非本批次责任）。

**关键对抗复核结论（独立验证 impl 的 patch-CAS 实证）：成立。** wiremock 实测确认 kube-rs JSON Merge Patch 请求体携带 `metadata.resourceVersion`，409 正确映射 `Conflict`；k3d v1.35.5 真 apiserver 复证（成功写后 rv 前进、过期 rv 409）——**Phase 2 RBAC 维持 get/patch 写路径、无需 update verb 的决策成立**。盲写风险已从代码层封死（`patch_data` 的 `resource_version` 为非空必传参数，store `save()` 仅接受 `ConfigVersion::Kubernetes`，不存在无 rv 写路径）。

待修复：**S2×2 + S3×6**（详见下），其中 S2-2 直接对应 impl 已知偏差 #1（写回时间戳为内存态）。

---

## Findings

### S2-1（重要）PG advisory lock 连接使用 `NoTls` —— 凭据/查询在同节点桥接上明文

【证据】
- `crates/ponyllm-server/src/refresh_lock.rs:69`：`tokio_postgres::connect(&dsn, NoTls)`；:22 `use tokio_postgres::NoTls`。
- 拓扑实况（T0 核查）：集群 flannel vxlan 运行在 `tailscale0` 上（tencent config.yaml），**跨节点** Pod 流量经 VXLAN-over-WireGuard 加密；但**同节点** Pod 间流量在 cbr0 桥接上为明文 L2。
- PG startup 消息（含 user/password）在非 TLS 下明文发送；advisory lock 查询本身无敏感数据，但连接凭据有。

【问题】
- 若锁库 Pod 与某网关副本同节点，同节点其他 Pod（或节点上任意进程）可嗅探 cbr0 捕获 lock DB 的 user/password（凭据随 startup 明文）；若未来锁库外置（多集群/DR），则公网明文。
- 凭据为 lock-only 角色（CONNECT-only，价值有限）且 env 泄露面（被攻破 Pod 可读 env）更大，故不构成 S1；但"凭据明文上线"是防御纵深缺口，且修复成本极低。

【修复建议】
- 首选：`tokio_postgres::connect_tls` + rustls（`tokio-postgres` 支持 rustls 通道），pg_hba 对网关来源强制 `scram-sha-256` + `hostssl`。
- 次选：显式文档化"依赖 Tailscale WireGuard 覆盖层加密 + 锁库与网关非同节点调度约束"，并加启动告警（DSN host 与 `PONYLLM_NAMESPACE` 节点不一致时 warn）。
- 任一方案都建议加一条测试断言：连接错误串/日志不含 DSN 明文（见 S3-3）。

---

### S2-2（重要）invalid_grant 跨副本缓冲缺口 + 写回失败时 freshness guard 失效（对应 impl 偏差 #1）

【证据】
- `state.rs` keepalive 的 invalid_grant 缓冲（`perform_antigravity_keepalive_cycle`）：仅当**本进程** `last_antigravity_refresh` 有该 key 且 < 60s 才延期隔离；跨副本"另一副本刚轮换"无法感知。
- `apply_token_freshness_guard`（state.rs:707-749）：只在 `last_antigravity_refresh` 有记录时保护内存 token；而该记录只在 persist **成功**（或 token 未变跳过写）时写入——**刷新成功但写回失败（`refresh_persist_failure_total`>0）时无记录**。
- 写回只写 token、不带时间戳进 Secret（impl 已知偏差 #1："写回时间戳为内存态"）→ 副本无法通过 Secret 内容判断"别人才轮换过"。

【问题】
- 场景 A（传播窗口）：副本 A 持锁刷新并轮换 refresh_token（Google 对部分客户端已启用轮换并作废旧 token），副本 B 的 keepalive 恰在 A 写回后、B 2s 轮询前的窗口内用旧 token 刷新 → `invalid_grant`；B 无本进程 recent-persist 记录 → **立即永久隔离该 key**（`AuthInvalid` → key 停用，需人工恢复）。
- 场景 B（写回失败）：A 刷新成功但写回 3 次重试仍失败 → 无 freshness 记录 → A 的 2s 轮询 rebuild 时旧 Secret 覆盖内存新 token → A 回退旧 refresh_token → 下次刷新 `invalid_grant` → 永久隔离。
- 两场景概率低（小时级周期 × 秒级窗口），但失败模式为**永久隔离**，且 7 天观察期指标（刷新成功率>95%）可能掩盖单 key 静默死亡。

【修复建议】
- 写回时在 Secret 配置内附 `rotated_at`/时间戳标记（或单独 data key `refresh-meta`），使"轮换时间"成为集群可见状态——直接解决偏差 #1，且能让所有副本的缓冲判定基于 Secret 侧时间。
- 刷新前守卫（不依赖时间戳即可实现）：`get_valid_token` 慢路径刷新前比较"内存 refresh_token vs 最近一次 Secret snapshot 的 token"，不一致且内存 token 更新 → 先 reload 再刷。
- persist 失败（或任何上游刷新成功）也应写入 `last_antigravity_refresh`（token 仍比 Secret 新），使 freshness guard 与缓冲覆盖场景 B。
- 保留现有"invalid_grant 需连续 N 次（非首次）才隔离"的兜底可选项。

---

### S3（建议）

**S3-1 gate 依赖 env 存在性——漏配即静默降级并发**
【证据】CLI 仅当 `PONYLLM_LOCK_DATABASE_URL` 非空才注入 `PostgresRefreshLock`（main.rs）；gate 为 `None` 时管理器直刷（antigravity.rs `None => None` 分支）。
【问题】4 副本中任一副本漏设 env → 该副本无门卫，同 IP 并发刷新风险回归，且无任何告警（只有 `refresh_lock_acquired_total` 恒 0 可旁证）。
【修复建议】启动日志显式打印"refresh serialization enabled/disabled"（已有 enabled 分支，补 disabled 告警）；Phase 4 验收项"任意时刻仅一个执行者/刷新并发为 0"必须含指标断言 `refresh_lock_acquired_total > 0`（多副本模式下）。

**S3-2 PG connect 错误串脱敏**
【证据】`refresh_lock.rs:79` `format!("refresh lock PG connect failed: {}", e)`；`tokio-postgres` 连接错误 Display 通常含 host:port（不含密码），但解析类错误（Config error）可能含更多。
【问题】错误串进日志；即使不含密码，host/user 信息也无必要。
【修复建议】统一对 connect/parse 错误只记 `kind`（如 "connect failed: connection refused"），不展开原始 error；加单测断言 DSN 子串不出现在错误 Display。

**S3-3 wiremock 测试 kubeconfig 使用 `insecure-skip-tls-verify`**
【证据】`kubernetes_store_wiremock_tests.rs` 测试 kubeconfig 带 `insecure-skip-tls-verify: true`（针对 wiremock HTTP 端点）。
【问题】测试桩专用，生产 `kube::Config::infer()` 全校验（in-cluster CA / kubeconfig CA），`k3d-smoke.sh` 用 k3d kubeconfig（真 CA）——无实际风险；但该模式易被复制进生产配置。
【修复建议】保持测试隔离并在注释中显式标注"禁止用于生产/Phase 2 部署清单不得出现 insecure-skip-tls-verify"；Phase 2 RBAC 验收增加"证书校验开启"自测项。

**S3-4 drain 未门控 admin CUD 写路径**
【证据】`is_draining` 覆盖：config poller（停止）、keepalive（跳过）、persist hook（抑制）；admin.rs 的 CUD 保存路径无 drain 检查。
【问题】drain 窗口（≤60s）内用户写仍可执行并落 Secret（持久、不丢，其他副本轮询可收到），但响应可能随 Pod 终止丢失 → 客户端按 412/超时重试，语义可接受。
【修复建议】可选：drain 期间 admin 写返回 503（与 `admin_store_unavailable` 同语义）；至少 Phase 2 文档注明该行为。

**S3-5 kube 依赖的 audit 门禁仍未加**
【证据】Cargo.toml 精确声明 kube 0.95 / k8s-openapi 0.23（v1_31）；Cargo.lock 已锁（kube 0.95.0 / k8s-openapi 0.23.0 / kube-client 0.95.0）；cargo tree 确认纯 rustls 链（hyper-rustls 0.27.9 → rustls 0.23.43，无 openssl/native-tls，仅 openssl-probe 路径探测）；CI 无 `cargo audit`。
【问题】供应链面（T0 S3-1）仍开放。
【修复建议】Phase 2 前置把 `cargo audit`（+可选 deny）加进 CI 门禁；发布镜像补 SBOM。

**S3-6 T0 两项 S1 前置沿用（非本 diff 新增，进度追踪）**
【证据】T0 S1-1（flannel 不执行 NetworkPolicy）、S1-2（静态加密未启用 + etcd 明文快照）已由 d16ac5c 采纳；本 diff（crates/scripts 代码）不含集群级修复。
【问题】Phase 2 上线前仍须完成（含 apiserver 公网暴露面收敛、audit 启用）。
【修复建议】Phase 2 前置清单显式挂这两项；`ponyllm-live-config` 写入（antigravity 写回）上线前静态加密必须已启用。

---

## 对抗复核确认项（T2 清单逐条结论）

| # | 复核项 | 结论 |
|---|---|---|
| ① | kube-rs in-cluster 配置 / TLS / 盲写 | **通过**。`Config::infer()` 用 in-cluster CA 全校验（生产代码 grep 无 accept_invalid_certs/danger 标记）；`patch_data` 强制携带 resourceVersion（非空参数），store 层无无 rv 写路径；wiremock + k3d 双重实证。SA token 可读性依赖部署层 fsGroup（Phase 2 保留 `fsGroup: 10001`，已列入清单） |
| ② | 冲突映射信息泄漏 | **通过**。412 响应为通用文案（无 rv/Secret 内容/apiserver 错误体）；409 的 expected/current rv 仅进服务端日志（warn 文案不含 rv）；404/500 映射为 InvalidData/Io，客户端文案通用 |
| ③ | PG advisory lock 闭环 | **通过（S2-1 除外）**。专用连接持锁全程（guard 生命周期=刷新+写回）、60s 查询超时、断连自动释放、连接错误/查询错误/超时三路径均 fail-closed 且计 `refresh_lock_error_total`；DSN 仅 env、Debug 脱敏、不落盘、构造期预热身无副作用；`pg_try_advisory_lock(hashtext($1))` 全副本同 key 确定性串行 |
| ④ | drain 丢写 | **通过**。shutdown watch → poller 停止 / keepalive 双检查跳过 / persist hook 入口抑制；drain 上限 60s < 180s grace − 25s preStop = 155s 余量（Phase 2 实测确认，偏差 #4 成立）；写回已持久化至 Secret，drain 不丢已落盘写 |
| ⑤ | 刷新写回闭环 | **通过（S2-2 除外）**。锁内写回、3 次有界重试+指数退避、`refresh_persist_failure_total`、语义单键变更（整文件 CAS replace，无盲写）、token 未变跳过写；invalid_grant 缓冲 + rebuild freshness guard（60s）已实现，但跨副本窗口与 persist 失败场景存在 S2-2 缺口 |
| ⑥ | 依赖面 | **通过**。kube 0.95.0 / k8s-openapi 0.23.0（v1_31 feature，匹配集群 k3s v1.31.6）/ kube-client 0.95.0；纯 rustls 链（rustls 0.23.43，无 openssl/native-tls）；Cargo.lock 提交锁定。剩余：CI cargo audit 门禁（S3-5） |

---

## 采纳清单建议（映射阶段）

| 优先级 | 采纳项 | 落入阶段 |
|---|---|---|
| S2 | PG 锁连接启用 rustls TLS（或显式 overlay 加密依赖文档 + 拓扑约束 + scram 强制） | Phase 1 收尾 / Phase 2 前置 |
| S2 | 写回附集群可见 `rotated_at` 标记（修 impl 偏差 #1）；刷新前"内存 vs Secret token"不一致守卫；persist 失败也记 freshness；invalid_grant 连续 N 次才隔离 | Phase 1 收尾 |
| S3 | 漏配 gate 告警（多副本下 `refresh_lock_acquired_total==0` 告警）+ 启动日志 disabled 提示 | Phase 1 收尾 |
| S3 | PG connect 错误脱敏 + DSN 不出现在错误串的单测断言 | Phase 1 收尾 |
| S3 | wiremock insecure-skip-tls-verify 仅限测试、生产禁止（部署清单校验项） | Phase 2 |
| S3 | drain 期间 admin 写返回 503（可选）或文档化重试语义 | Phase 2 |
| S3 | CI 加 cargo audit + 镜像 SBOM | Phase 2 前置 |
| S1（沿用） | T0 S1-1（CNI/Cilium + netpol 实测）、S1-2（静态加密启用 + 快照加密）、apiserver 暴露面收敛 | Phase 0/2（非本 diff） |

## 审核限制

- k3d 真集群 CAS 实证由 impl 的 `kubernetes_store_k3d_tests` + `scripts/k3d-smoke.sh` 执行（需 docker+k3d，本次环境未跑，代码与脚本已审）；wiremock 实证本次已本机复跑全绿。
- PG NoTls 的实际暴露取决于锁库最终部署拓扑（同节点/跨节点/外置），Phase 2 定稿拓扑后需按 S2-1 复核。
- 静态加密/CNI/audit 等集群级项不在本 diff，沿用 T0 报告结论与验收清单。
