# Phase 4 调优定向对抗安全审核报告（红队 / sec-reviewer）

- 审核对象：Phase 4 文档 T27 调优增量（commit 59982c5：`docs/phase4-cleanup.md` + `docs/phase4-observation.md`）
- 审核日期：2026-09-29
- 审核方式：只读（diff 审阅 + 集群实况验证 last-applied 注解内容结构）；无任何集群写操作
- 审核范围：① 备份 umask/chmod/stat 权限断言；② `{.metadata}` 降级是否杜绝凭据裸存；③ 30 天删除命令完整性；④ A10-3/A10-4 是否纳入观察框架

## 总体结论：**有条件通过（S2×2）**

T27 对 P4 S2-1 的修复大部分落地（umask 077 + chmod 700 + Secret 改 jsonpath 子集 + 30 天删除 + A10-3/A10-4 纳入），但发现 **2 处 S2 级缺陷**：① 权限断言的**逻辑倒置**（备份目录失守时反而不 FAIL）与文件权限 644 豁免；② **`{.metadata}` 子集并未杜绝凭据裸存**——`ponyllm-config` 带 `kubectl.kubernetes.io/last-applied-configuration` 注解（实测存在、16163B、**内含 `ponyllm.toml` data 键**），该注解随 `{.metadata}` 一起被备份，8 组 provider key 的 base64 明文**依然裸存**。③④ 本身落点正确（③命令完整但置于注释位，④已纳入但 A10-3 的 kubectl debug 缺授权标注，均 S3）。

## 复核清单逐条结论（lead 四项）

| # | 复核项 | 结论 |
|---|---|---|
| ① | 备份 umask 077 + chmod 700 + stat 权限断言（失守 exit 1） | **缺陷（S2-1）**。`umask 077` + `chmod 700 "$BK"` 正确；但断言 `stat -c '%a' "$BK" \| grep -qx 700 && … grep -vqE '^600$\|^644$' && echo FAIL && exit 1 \|\| true` 有三处问题：(a) **逻辑倒置**——目录非 700 时 `grep -qx 700` 失败 → `&&` 链短路 → `\|\| true` 兜底 → **不 exit 1**，即"备份目录失守"这一最该拦截的场景被放行；(b) 文件豁免含 **644**（`-v` 反转后 600\|644 均放行）——umask 077 下正常只应产生 600，644 说明 umask 失效却被放行；(c) `\|\| true` 吞掉未落 FAIL 路径 |
| ② | `{.metadata}` 降级杜绝 provider keys 明文 base64 裸存 | **缺陷（S2-2，实况确认绕过）**。`kubectl get secret ponyllm-config -o jsonpath='{.metadata}'` 包含 `metadata.annotations`；实况验证 `ponyllm-config` 带 `kubectl.kubernetes.io/last-applied-configuration` 注解（**16163B，且内含 `ponyllm.toml` data 键**——client-side apply 创建时整对象入注解）→ 备份的 `secret-ponyllm-config.metadata.json` **仍含全部 provider keys 的 base64 明文**，"禁止裸存"承诺未达成 |
| ③ | 30 天删除命令完整 | **通过（S3 微调）**。`find /root/ponyllm-phase4-backup-* -maxdepth 0 -type d -mtime +30 -exec rm -rf {} +` 语法/语义完整（maxdepth 0 防递归误删、-mtime +30 期限、多目录逐个删）。**S3**：命令置于注释行（`# 30 天后…`），建议移到可复制执行位（§4 后置节） |
| ④ | A10-3 emptyDir 巡检 + A10-4 rbac-audit 首末 diff 纳入 | **通过（S3 微调）**。已纳入判定表：A10-3（节点 emptyDir/旧路径凭据残留巡检，期望空）+ A10-4（rbac-audit.sh 首日/第 7 天各跑一次、diff 必须空），观察日志模板已加 `node_residue=0 / rbac_audit_diff=empty` 字段。**S3**：A10-3 用 `kubectl debug node/<节点> --image=busybox …`（**创建 debug Pod = 集群写操作**）未标注"需 Lead 授权"；且 k3s 节点的 debug 支持不保证。建议改 `ssh <node> find /var/lib/kubelet/pods …`（只读直连）或显式标注授权 |

---

## Findings

### S2-1（重要）备份权限断言语义倒置：目录失守时不 FAIL，且 644 被豁免

【证据】cleanup.md §0 断言：`stat -c '%a' "$BK" | grep -qx 700 && {…校验文件…} && echo "FAIL 备份权限异常" && exit 1 || true`。
【问题】
- 目录 mode ≠ 700 → 第一步 grep 失败 → 链短路到 `|| true` → **不 exit 1**——恰好是"备份目录权限失守"时应 FAIL 的场景被放行；守卫只在"目录恰好 700 **且**存在异常文件"时触发。
- 文件豁免集 `{600,644}` 弱于"备份文件=600"承诺（P4 S2-1 原意）：umask 077 下不可能产生 644，出现 644 即 umask 失效，却被放行。
【修复建议】改为独立正向断言：
```bash
[ "$(stat -c %a "$BK")" = "700" ] || { echo "FAIL 备份目录权限 != 700"; exit 1; }
find "$BK" -type f ! -perm 600 -print -quit | grep -q . && { echo "FAIL 备份文件权限 != 600"; exit 1; }
```
（`! -perm 600` 严格 600，无 644 豁免；任一失守即 exit 1。）

### S2-2（重要）`{.metadata}` 备份经 last-applied 注解仍携带全部凭据明文

【证据】实况：`ponyllm-config` 存在 `kubectl.kubernetes.io/last-applied-configuration` 注解（16163B）；该注解值内含 `ponyllm.toml` data 键（grep count=1）——client-side apply 遗留的整对象快照（含 data base64）。T27 备份命令 `-o jsonpath='{.metadata}'` 将该注解一并导出。
【问题】`secret-ponyllm-config.metadata.json` 并非"仅元数据"——8 组 provider key 的 base64 明文经注解通道**依然裸存**，"杜绝明文 base64 裸存"的承诺未达成（文件权限 600 未能减轻"不该存在明文"的违反）。
【修复建议】备份子集去掉 annotations（或显式排除 last-applied 键）：
```bash
kubectl -n ponyllm get secret ponyllm-config \
  -o jsonpath='{.metadata.name}{"\n"}{.metadata.creationTimestamp}{"\n"}{.metadata.resourceVersion}{"\n"}{.metadata.labels}'
```
（只留名称/时间/版本/标签；或 `-o jsonpath='{.metadata.annotations}'` 后 `jq 'del(."kubectl.kubernetes.io/last-applied-configuration")'`。）

---

### S3（建议）

**S3-1 ③ 删除命令移入可执行位**：`find … -mtime +30 -exec rm -rf {} +` 完整但位于注释；移到 §4 或新增"备份到期清理"小节，使其可直接复制执行（并加 `set -euo pipefail` 语境说明）。

**S3-2 ④ A10-3 授权标注与只读化**：`kubectl debug node/` 会创建 debug Pod（集群写），且 k3s 节点 debug 支持不保证；标注"需 Lead 授权"或改 `ssh <node> 'find /var/lib/kubelet/pods -maxdepth 6 -name ponyllm.toml -o -name "*.json"'`（只读直连，无 Pod 创建）。

**S3-3 规范化校验**：T27 其余落点（A7-1 极性修复 `! grep -qE '=[1-9]'`、A7-3 剔除 skipped、A7-4 活动窗口采样 + skipped 正面证据 + 主动触发 keepalive、A7-2 双阈值）由 qa/arch 复核，无新增安全面。

---

## 采纳清单建议

| 优先级 | 采纳项 | 落入阶段 |
|---|---|---|
| S2 | 备份权限断言重写为独立正向检查（目录 700、文件严格 600，失守即 exit 1） | Phase 4 清理执行前（文档修订） |
| S2 | `{.metadata}` 备份剔除 last-applied 注解（改 name/时间/版本/标签子集或 del 注解） | Phase 4 清理执行前（文档修订） |
| S3 | 30 天删除命令移入可执行位 | 随 T28 修订 |
| S3 | A10-3 改 ssh 只读 find 或标注 Lead 授权 | 随 T28 修订 |

## 复核确认项

- umask 077 + chmod 700 + Secret yaml → jsonpath 子集的方向正确（P4 S2-1 主修复采纳）。
- ④ A10-4 rbac-audit 首末 diff 落点正确（注意 rbac-audit.sh 运行需 admin 凭证，观察期执行者具备）。
- 观察日志模板新增 `node_residue`/`rbac_audit_diff` 字段，符合"token 不落日志"纪律。

## 审核限制

- 只读审阅 + last-applied 注解结构实况验证（未打印注解值内容，仅计数 data 键出现次数）。
- ① 断言的 shell 求值语义按 bash `&&`/`||` 短路规则静态推导，未执行该备份命令（写操作）。