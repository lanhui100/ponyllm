# T23 GitOps 调优定向复核报告（架构红队）

- 审核对象：T22 调优增量 3 commits `8b73e8c..fec56f5`（diff `169acfd..HEAD`：`docs/gitops-pipeline-runbook.md` + `scripts/release-gate.sh` + ADR 链路事实重构 + 旧笔记指针）
- 审核人：arch-reviewer
- 聚焦：① 宿主侧冒烟命令可执行性与清理保证 ② 迁移 ADR `## 链路事实` 改指针+时点快照后 digest 常量是否消除、节序是否合规 ③ 单一真相源（取号命令/回滚引用后无跨文件重复）
- 核查方式：diff 走读 + CLI 源码核对 + `verify-note.sh` + `bash -n` + 只读 kubectl/curl（无集群写）

---

## 总体结论：**通过**（T20-arch 的 S2-1/S2-2 与 S3 大部闭环；冒烟命令逻辑经源码核对成立并含清理保证；仅余 1 条 S3 残余，不阻断）

宿主侧冒烟命令依赖的 `ponyllm init --non-interactive --output` 在 CLI 中真实存在（cli.rs:19 / main.rs:723），`/health` 实际返回体 `{"status":"ok"}` 已由 live service 实测一致；ADR 的 `## 链路事实` 整节删除、事实收拢到 Consequences 的"单一真值源声明 + 2026-09-29 时点快照（非权威）"，digest 常量已从 runbook/ADR 双处移除，节序恢复规范（Decision → Alternatives → Consequences）。

---

## 一、聚焦逐条核验

### ① 宿主侧冒烟命令可执行性与清理保证 —— 通过
【证据】runbook §3 第 3 步（T22 重写）：
`docker run --rm -d --name pg-smoke --entrypoint sh -p 127.0.0.1:18080:8080 <镜像> -c 'ponyllm init --non-interactive --output /tmp/ponyllm.toml && exec ponyllm serve --bind 0.0.0.0:8080 --config /tmp/ponyllm.toml' && sleep 3 && curl -sf http://127.0.0.1:18080/health`，结束清理：`docker rm -f pg-smoke`（curl 失败也清理：`RC=$?; docker rm -f pg-smoke >/dev/null; exit $RC`）。
【核验】
- **子命令/参数真实存在**：`crates/ponyllm-cli/src/cli.rs:19` `Init { output, non_interactive }` + `main.rs:723` 派发——`--non-interactive`/`--output` 非虚构；`init` 在容器内 /tmp 生成一次性配置（不挂真实 Secret），`serve --config` 与之衔接；
- **入口覆盖**：镜像 `ENTRYPOINT ["ponyllm"]`，`--entrypoint sh` + `-c` 正确以 sh 执行 init→serve 序列，`exec` 使信号直达 ponyllm；USER 10001 下 /tmp 可写；
- **宿主侧验证**：`-p 127.0.0.1:18080:8080` 仅本机暴露 + 宿主 `curl -sf`（devserver 有 curl，T19 已实证）；
- **健康体断言**：T22 声称冒烟返回 `{"status":"ok"}`——live 实测 `curl http://10.43.30.21:8080/health` 返回 `{"status":"ok"}`，与断言一致；
- **清理保证**：`--rm` 容器退出自清 + 失败路径 `RC=$?; docker rm -f; exit $RC` 显式保留退出码（runbook 行内注明）——对比 T20 版本（in-container wget||curl 恒 127）已根本修复。
【S3 残余（不阻断）】该清理行是"人工后续执行"的说明而非单条原子命令；若做成一行自动完成（`docker run … && sleep 3 && (curl -sf …; RC=$?; docker rm -f pg-smoke >/dev/null; exit $RC)`）可防操作者漏清理，建议并入 §3 检查表（纯措辞，可选）。

### ② ADR `## 链路事实` 改指针+时点快照 —— digest 常量已消除、节序合规
【证据】fec56f5：
- `## 链路事实（2026-09-29 实测校准）` 整节**删除**（原含 `sha256:3bfad2f9…` 常量与 keel/GHCR 复述）；
- 事实收拢到 `## Consequences`：首条"链路事实以 docs/gitops-pipeline-runbook.md 为准（单一真值源）；易变 digest 等常量不在此笔记维护，现网 digest 取号命令见 runbook §2"；随后以"2026-09-29 时点快照（仅快照，非权威）"给出坐标 + 取号命令 + Keel 引用，**无任何 digest 常量**（`grep 3bfad2f9|b1788e90` 在 runbook/ADR 双文件零命中）；
- 节序恢复规范骨架：`## Problem` → `## Decision` → `## Alternatives considered` → `## Consequences`（无插队章节）；
- `bash .agents/skills/write-adr/verify-note.sh …` → "全部通过（机械影子 1.1–1.8）"。
【核验】T20 S2-2（ADR 复述 = 第二漂移点）关闭：digest 常量双处清零，事实以指针 + 带"非权威"标注的时点快照呈现，节序合规。

### ③ 单一真相源 —— 无跨文件重复，收敛完成
【证据】
- runbook §2：现网运行 digest 改为**取号命令**（`kubectl get deploy … -o jsonpath image`，"本手册不写死易变 digest（单一真值源在集群）"）；历史回滚 digest 改为**引用** `deploy/ponyllm-phase2-rollback.md` R2（"本手册不重复"）；
- release-gate.sh T22：`--rollback-digest` 改为**必填**（删除原硬编码回退 `b1788e90`，附取舍说明"静默回退可能滚到过期/错误 digest"）；新增 `docker manifest inspect` 远端可解析校验（防"本地有 tag、registry 未同步"假阳性）；digest 格式前置于 target+rollback 双校验（qa S3-3）；
- 旧笔记指针：`2026-09-28-zero-downtime-rolling-update-and-pull-based-cd.md` 顶部新增"发布链路事实以 runbook 为准（本笔记历史描述与实测链路存在偏差，见 runbook §5）"——历史 GHCR 描述不再被误当现行事实；
- grep 证实：3bfad2f9 / b1788e90 在 runbook 与迁移 ADR 中零引用（唯一残留为 rollback 文档 R2 的权威定义 + deployment 清单的当前值，均属各自单一真值位）。
【核验】digest 类易变常量收敛为"集群取号 + 单一权威文档引用"，不存在跨文件复述；`bash -n scripts/release-gate.sh` 通过。

---

## 二、残余 S3 建议（不阻断）

1. **S3-1** 冒烟清理行建议合并为单条原子命令（见聚焦①），防漏清理。
2. **S3-2** 发布机制仍为 `kubectl set image`（T20 S3-3 部分采纳：新增第 6 步"运行镜像==目标 digest 复核"弥补）；由于 release-gate 第 4 步强制"清单文件已引用目标 digest"，文件==线上在发布时点成立，漂移仅发生于事后绕过清单直接改线上——可接受的失守面，建议后续发布改用"更新清单 digest → `kubectl apply`"彻底收敛。

---

## 三、采纳清单建议

- **无必改项**：聚焦①②③全部闭环（冒烟可执行、digest 常量清零、节序合规、单一真值源收敛），`verify-note.sh` 1.1–1.8 与 `bash -n` 均过。
- **可选**：S3-1/S3-2 随下次发布流程维护处理。

---

## 复核命令（只读，本报告已执行）

```bash
bash .agents/skills/write-adr/verify-note.sh .agents/notes/implemented/process/2026-09-29-gitops-pipeline-runbook.md  # 1.1-1.8 全过
bash -n scripts/release-gate.sh && bash -n scripts/phase3-verify.sh          # 语法通过
grep -rn "3bfad2f9\|b1788e90" docs/gitops-pipeline-runbook.md .agents/notes/implemented/process/2026-09-29-gitops-pipeline-runbook.md  # 零命中
grep -n "Init {\|non_interactive\|output" crates/ponyllm-cli/src/cli.rs      # init --non-interactive --output 真实存在
curl -s http://10.43.30.21:8080/health                                        # {"status":"ok"}（与冒烟断言一致）
docker run --rm -d --name pg-smoke --entrypoint sh -p 127.0.0.1:18080:8080 <镜像> -c 'ponyllm init --non-interactive --output /tmp/ponyllm.toml && exec ponyllm serve --bind 0.0.0.0:8080 --config /tmp/ponyllm.toml' && sleep 3 && curl -sf http://127.0.0.1:18080/health   # 发布前 dry 跑（T22 已实测 {"status":"ok"}）
```