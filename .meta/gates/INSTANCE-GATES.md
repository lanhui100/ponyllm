# 实例门登记（本仓自有；不属框架骨架文件）

> 为什么单独一个文件：`.meta/gates/README.md` 在 ponygo 的 `UPGRADE_UNIVERSE` 内（逐字节锁定），
> 往里追加实例门会让 `ponygo upgrade --check` 长期报漂移。框架 README 只登记 A1–A9；
> 实例门与「承诺 → 命令」写在这里，既不制造骨架漂移，也不把门禁藏进无人看的地方。

| 门 | 脚本 | 负样本 spec | 接线 | 退出语义 |
|---|---|---|---|---|
| backlog 账本门 | `.meta/gates/backlog-gate.sh` | `.meta/gates/backlog-gate.spec.sh` | `.meta/gates/pre-commit` 的 `# >>> backlog-gate: ledger v1 >>>` 哨兵块 + `.github/workflows/ci.yml` 的 `backlog-ledger` job | 0 放行 / 非零拒绝；账本缺失 → 明确跳过；无 `node` → 拒绝（fail-closed） |

## 承诺 → 非零退出命令

| 承诺 | 命令 | 命中判据 |
|---|---|---|
| 账本记录合法且跨归档/租约对账干净 | `bash .meta/gates/backlog-gate.sh` | `lint`（必填字段/条件必填/状态字段合法性/租约-状态互斥/活跃池 done 残留/ID 唯一/高水位）+ `doctor`（ID 重复/高水位/孤儿租约/租约-状态互斥） |
| 门自己会拒绝非法样本 | `bash .meta/gates/backlog-gate.spec.sh` | P1 放行 + N1–N8（缺 fingerprint / 活跃 done / 孤儿租约 / 高水位越界 / 无账本跳过 / 无 node 拒绝 / 锁存而记录非 in_progress / 真实接线提交非法账本被拒）+ H（pre-commit 接线：门在场放行 / 门缺失 fail-closed） |

## 契约与真源

- 可读契约：`backlog/README.md`（布局/字段/状态机/租约/归档/命令）
- 判定真源：`scripts/backlog.mjs`（`validateRecord` / `lint` / `doctor`）——本目录脚本只编排，不复刻判据。
  复制自框架基线仓（pony-agent）：`scripts/backlog.mjs`（md5 `902bf84ad5516009b0172260b71575eb`，758 行）与
  `backlog-gate.sh`（md5 `0a5bbdb7e12e70ffc6934be70e3bbb6c`）与本仓文件**逐字节一致**；
  `backlog-gate.spec.sh` 的基线 md5 是 `8e7dd74323bb1c958c6f1194f88e4489`，本仓= `e2240d7867303896b3525ef56f37ca58`
  ——差异只在 H 段（本仓 hooksPath 是 `.meta/gates`，H 改为从真实 hook 的哨兵块抽取接线片段）与新增的 N8 段
  （非法账本经真实接线必须被拒；H 单独无法发现接线被改成 fail-open）。P1 / N1–N7 一字未改。
- 诚实边界：spec 只证明「门会拒非法样本」；账本内容真实性靠 review。`doctor` 不解析 git 历史，
  无法识别「同时改记录与高水位」的伪装。
- 本地钩子前提：`git config core.hooksPath .meta/gates`（git 不随克隆携带该配置；本仓已按既有方式配置）。
  未装配时本地门禁静默失效，CI 的 `backlog-ledger` job 仍兜底——这是 git 的机制限制，不是门禁缺陷。

## 已退休的门（保留登记，防止"消失了没人知道"）

| 门 | 原脚本 | 退休日期 | 原因与去处 | 残留收据 |
|---|---|---|---|---|
| commit-msg 任务 ID 钩子 | `.meta/gates/commit-msg` + `commit-msg-task-id.{sh,spec.sh,spec.ps1}` | 2026-11-12 | 同一 2026-09-06 任务卡治理栈的遗留：只在 `task/WEB-*` 分支要求提交信息含 `WEB-xx`，而该前缀分支已无生成路径（卡体系退役）→ 属死接线，随栈一并删除；`core.hooksPath=.meta/gates` 下无同名文件即自动失效 | 无活接线（靠 hooksPath 按文件名发现，删除后零引用） |
| 任务卡体系门 | `.meta/gates/check-tasks.{sh,ps1,spec.sh,spec.ps1}` | 2026-11-12 | 任务卡体系（`.agents/tasks/web-console/`，Active 五列全空、9 卡已归档）整体退役：状态真源统一为 `backlog/tasks.jsonl`（见决策记录 `2026-11-12-backlog-v2-jsonl-ledger.md`）。原文归档进 `backlog/legacy/`，`manage-tasks` 技能同时冻结；接线已从 `.meta/gates/pre-commit` 与 `.github/workflows/ci.yml` 移除 | `bash .meta/gates/no-residue.sh`（清单 4 条：`check-tasks.sh` ← CI/hook、两张旧板的路径 ← `.gitattributes`） |

退休门的判据不再有消费者；`check-tasks` 这个名字若将来重新出现，必须先在 `backlog/` 立新记录并重跑
`no-residue` 清单（清单是删除的收据，不是装饰）。
