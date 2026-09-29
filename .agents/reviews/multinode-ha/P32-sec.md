# P3.2 T13 调优第二轮定向对抗安全审核报告（红队 / sec-reviewer）

- 审核对象：T13 调优第二轮增量（3 commits: b4d7562, d0c261a, 7632085；diff 9cf7482..6480948 限 deploy/ 与 scripts/）
- 审核日期：2026-09-29
- 审核方式：只读（提交 diff 审阅 + 与 P2 生产实况矩阵比对）；无任何集群写操作
- 审核范围：① R1 卷补丁播种源（live-config / 移除 config-ro）+ PVC 重建重播种演练断言；② `scripts/rbac-audit.sh` 矩阵完整性；③ 回滚凭据面（旧凭据写回/陈旧读取是否堵死）

## 总体结论：**通过（无 S1/S2）**

T13 三项 sec 落点（P31 S2-1 + S3-1 + 回滚凭据面）**全部闭环**：R1 播种源已改 `ponyllm-live-config` 并新增 PVC 重建重播种演练（sha256 逐字节断言）；`rbac-audit.sh` 以 **18 项**矩阵（超出建议的 16 项）非零退出校验最小权限；回滚路径"旧凭据写回"与"陈旧读取"两条通道均被堵死。仅 S3×2（运行前置权限说明、演练断言硬编码哈希的动态化建议）。

## 复核清单逐条结论（lead 三项）

| # | 复核项 | 结论 |
|---|---|---|
| ① | R1 卷补丁播种源 + PVC 重建重播种演练 | **通过**。R1 卷补丁 `secretName` 已由 `ponyllm-config` 改为 **`ponyllm-live-config`**（deploy/ponyllm-phase2-rollback.md:65），R0 前置注释明确"绝不切回陈旧 ponyllm-config(125)"（:56）；回滚验收新增 **PVC 重建重播种演练段**：`rm -f` PVC 文件 → `rollout restart` 触发 init 播种 → `sha256sum /var/lib/ponyllm/ponyllm.toml == 90faddd…` 且与 live-config 解码哈希比对（:99-103）。S3 细节见 S3-2（硬编码哈希动态化） |
| ② | rbac-audit.sh 16 项矩阵完整性 | **通过（18 项，超建议）**。`scripts/rbac-audit.sh`：`expected\|verb\|resource\|ns` 结构化用例 18 条（yes×2 白名单 / no×16），覆盖同 ns 其余 4 个 Secret（ponyllm-config/aliyun-registry/lock-dsn/lock-tls/telemetry-snapshot）、整类 get/list/watch、create/update/delete、configmaps/pods/deployments、**跨 ns×2（kube-system + production）**；逐条 PASS/FAIL 输出 + `FAIL=1 → exit 1` 非零退出 + 期望值注释。**与 P2 生产实测矩阵逐字一致**。S3-1：运行前置权限需注明 |
| ③ | 回滚凭据面（旧凭据写回/陈旧读取堵死） | **通过（两条通道均堵死）**。**写回侧**：R1/R0 全程不写 Secret（FileConfigStore 与 persist hook 只写本地文件），live-config 保持真相源（rollback.md:4），无旧凭据写回路径。**读取侧**：R0 强制 `FORCE_CONFIG_SYNC=true`（或删文件）重播种 + R1 播种源 = live-config + PVC 重建演练 sha256 门 + `ponyllm-config`(125) 明确标记**废弃资产、禁止播种/更新、Phase 4 清理**（:106）——P31 S2-1 的"R1 之后 PVC 重建从陈旧 125 播种"场景已被三重机制堵死 |

---

## Findings

### S3（建议）

**S3-1 rbac-audit.sh 运行前置权限未注明**
【证据】脚本用 `kubectl auth can-i ... --as=system:serviceaccount:ponyllm:ponyllm-gateway-sa`——`--as` 需要调用者具备 **impersonate** 权限，`auth can-i` 需创建 SelfSubjectAccessReview/SubjectAccessReview（通常仅 admin/集群管理员）。
【问题】若用受限 operator 凭证运行会全部 FAIL（误报越权）或无法创建 SAR（误判环境失败）；执行者可能误以为 RBAC 回归。
【修复建议】脚本头部注释注明"需具备 impersonate + subjectaccessreviews create 的运维凭证（admin 级）"；或在失败时区分"SAR 创建失败"与"can-i 返回 no"（后者才真越权）。

**S3-2 PVC 重建演练断言硬编码哈希应动态化**
【证据】演练断言 `sha256sum ... # 90faddd675b1860ee04399c382bc253c7f3485b10a8617517567f34d1ad7f93c` 为固定值（rollback.md:100），比对命令（:102）现场计算 live-config 哈希。
【问题】若演练时 live-config 已变更（正常演进），固定 90faddd… 断言必失败——误报而非真实回归；硬编码哈希也随时间腐烂。
【修复建议】演练段改为"两条命令哈希相等"断言（或脚本化比对 `[ "$(pod sha)" = "$(secret sha)" ]`），去掉固定值；或演练前现场重算期望值。

---

## 采纳清单建议

| 优先级 | 采纳项 | 落入阶段 |
|---|---|---|
| S3 | rbac-audit.sh 头部注明运行前置权限（impersonate + SAR create），并区分 SAR 失败与 can-i=no | 随 T13 顺手 / Phase 3 执行前 |
| S3 | PVC 重建演练断言由"固定 90faddd"改为"pod sha == live-config sha 动态比对" | 随 T13 顺手 |
| 已闭环 | R1 播种源 live-config + 重播种演练 + ponyllm-config 废弃资产声明 —— 无后续动作 | — |

## 复核确认项（P31 采纳清单落点）

- P31 S2-1（R1 播种源）→ **已修复**（secretName=ponyllm-live-config + 演练断言 + 废弃声明）。
- P31 S3-1（auth 矩阵脚本化）→ **已落地**（rbac-audit.sh，18 项非零退出）。
- P31 S3-2（Phase 4 清理卡）→ 维持排期（runbook :106 已声明废弃资产归属 Phase 4）。
- 附加确认：verify.sh T13 修订（[4/7] Δ≤1 + `P3_EXPECTED_RELOADS` 对账、[6/7] 还原守卫 hash 比对防 clobber 外部写入、[7/7] FAILED=0/REFUSED≤2）无新增安全面；[6/7] 的写+还原属刻意写测试，须在受控窗口执行（脚本头已注明，QA 跟踪）。

## 审核限制

- 只读审阅提交 diff；rbac-audit.sh 未实际运行（运行需 admin 凭证 + 会创建 SAR 记录，属只读但需认证，未执行）；矩阵期望值与 P2 生产实测逐字一致，回归风险低。
- 回滚演练（rm/rollout）为集群写操作，未执行；仅审脚本与断言逻辑。
