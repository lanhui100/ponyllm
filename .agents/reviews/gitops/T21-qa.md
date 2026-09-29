# T21 质量/测试定向复核报告：GitOps T22 调优增量

- 审核对象：T22 调优增量（3 commits 8b73e8c/310c1a5/fec56f5；diff 169acfd..HEAD 限 `docs/gitops-pipeline-runbook.md`、`scripts/release-gate.sh`、`.agents/notes/implemented/process/*.md`）
- 审核角色：qa-reviewer（质量/测试对抗红队，只读）
- 审核日期：2026-09-29
- 独立验证（实测）：
  - `bash -n scripts/release-gate.sh` → OK（复跑通过）
  - 缺 `--rollback-digest` → **exit 2**（无硬编码回退）✅
  - 坏 digest 格式 → **exit 1**（check 0 双 digest 格式校验）✅
  - **ldd 输出格式 vs grep 模式实证**：真实格式 `version 'GLIBC_2.39' not found`（版本号后带撇号）；手册模式 `GLIBC_2.39 not found`（无撇号）→ `grep -q` **rc=1 未命中** → `! grep` 得 0 → **假绿**；宽松模式 `GLIBC_2.39` → rc=0 命中 ✅
  - 旧笔记指向（zero-downtime note 顶部 + GitOps ADR Consequences 时点快照）diff 核读通过

## 总体结论：有条件通过

T22 对 T20 的三条 S2 基本闭环：① 门禁 `--rollback-digest` 缺参 **exit 2 且无硬编码回退**（注释说明否决 kubectl 自动取的取舍）；③ 宿主侧冒烟（`-p 127.0.0.1:18080:8080` + 宿主机 `curl -sf` + 清理 RC 透传注记）修正"镜像无 curl/wget 必挂"。但 **② ldd grep 判据只修对了一半**：管道机制（`2>&1 | grep -q` + `!`）正确，**模式串 `GLIBC_2.39 not found` 与 ldd 真实输出（`GLIBC_2.39' not found`，版本号后带撇号）不匹配**——实证未命中 → 检查在 GLIBC_2.39 真缺失时仍假绿。另 2 条 S3 小项。S3 顺手项（digest 格式校验、tag 规范对齐、check 3 注释改实、jq 依赖、item 9 标"靠 review"、digest 去重、旧笔记指向、GHCR 全环境禁用、门禁层级显式化）全部落地。修完该 S2 残留后可放行。

---

## S1（阻断）：无

## S2（重要）

### S2-1 §3 检查 1（glibc ldd）grep 模式串与 ldd 真实输出不匹配 → 假绿残留

【证据】
- `docs/gitops-pipeline-runbook.md` §3 检查 1（T22 修订）：
  `! docker run --rm --entrypoint ldd <镜像> /usr/local/bin/ponyllm 2>&1 | grep -q 'GLIBC_2.39 not found'`
- ldd/ld-linux 缺版本诊断的真实行格式：`version 'GLIBC_2.39' not found (required by /usr/local/bin/ponyllm)`——**版本号与 "not found" 之间有一个闭撇号**。
- 本地实证：把该真实行喂给手册模式 `grep -q 'GLIBC_2.39 not found'` → **rc=1 未命中**；`! grep` 整体退出 0 → 检查 PASS。宽松模式 `grep -q 'GLIBC_2.39'` → rc=0 命中。

【问题】
机制修对了（`2>&1` 合并 stderr + `grep -q` + `!` 反转），但模式串缺少撇号 → GLIBC_2.39 真缺失时检查**仍假绿**，恰是检查要防的运行时崩溃场景。另：该模式硬编码 2.39（builder 侧可能随 rust 镜像演进出更高 glibc 需求），建议版本号通用化。

【修复建议】
模式改 ERE 兼容撇号并版本通用化：
`! docker run --rm --entrypoint ldd <镜像> /usr/local/bin/ponyllm 2>&1 | grep -qE "GLIBC_[0-9]+\.[0-9]+[^ ]* not found"`
（或简化为 `grep -qE "GLIBC_[0-9.]+[^ ]* not found"`；文档判据同步改为"无任何 GLIBC 版本缺失"）。修复后应做一次正/负向实测（造一个缺 GLIBC_2.39 的二进制或直接用 echo 行验证）。

---

## S3（建议）

### S3-1 release-gate.sh 与 runbook 文件尾缺换行
两文件 diff 均以 `\ No newline at end of file` 结尾。POSIX 文本文件应尾随换行（避免与某些工具/拼接场景出问题）。顺手补。

### S3-2 §3 检查 2（取 digest）未与门禁 TARGET_DIGEST 显式比对
`docker inspect --format '{{index .RepoDigests 0}}'` 输出带 registry 前缀的完整 digest，操作者需手动与 `--digest` 参数比对；门禁 check 2 已用本地 RepoDigests 精确匹配（`$IMAGE@$TARGET_DIGEST`），§3 检查 2 属冗余提示——建议注释注明"输出应等于 `$IMAGE@$TARGET_DIGEST`"即可，无需新增断言。

---

## 复核要点逐条落点（lead 聚焦）

| focus | 落点 |
|---|---|
| ① 门禁缺参 exit 2（无硬编码回退） | ✅ 实测 `--rollback-digest` 缺失 → `exit 2`；硬编码 b1788e90 回退已删除，注释说明取舍（否决 kubectl 自动取，因集群依赖+操作者不可察觉） |
| ② §3 检查 1 ldd grep 判据 | ⚠️ 机制正确（`2>&1 \| grep -q` + `!`），**模式串不匹配 ldd 真实输出（缺撇号）→ 假绿残留**（S2-1，实证） |
| ③ §3 检查 3 宿主侧冒烟 + 清理 RC 透传 | ✅ `-p 127.0.0.1:18080:8080` + 宿主机 `curl -sf`（不再依赖容器内 curl/wget）+ 清理注记 `RC=$?; docker rm -f pg-smoke >/dev/null; exit $RC`；实测声明 `{"status":"ok"}`（T22） |
| ④ TARGET_DIGEST 格式校验 + S3 顺手项 | ✅ check 0 对 target+rollback 双 digest 做 `^sha256:[0-9a-f]{64}$` 校验（实测坏格式 exit 1）；tag 规范对齐 `<semver>[-<suffix>]`；check 3 注释改实；jq 依赖入"依赖"行；§3 item 9 标"靠 review"；digest 去重（runbook 不再复述 b1788e90）；旧 zero-downtime note 顶部加指向；GitOps ADR 链路事实改"时点快照+指向 runbook"；GHCR 全环境禁用声明；§6 门禁层级显式化 |
| ⑤ bash -n | ✅ 复跑通过 |

## 采纳清单建议

| # | 建议 | 对应 | 优先级 |
|---|---|---|---|
| 1 | §3 检查 1 grep 模式改 ERE（`GLIBC_[0-9]+\.[0-9]+[^ ]* not found`）并做正/负向实测 | S2-1 | P0 |
| 2 | 两文件补尾随换行 | S3-1 | P3 |
| 3 | §3 检查 2 注明输出应等于 `$IMAGE@$TARGET_DIGEST` | S3-2 | P3 |