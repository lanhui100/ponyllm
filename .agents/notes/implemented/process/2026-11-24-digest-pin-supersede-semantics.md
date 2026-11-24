# Agent Note: 部署 pin 回写改「超越者让位」——消除连续合并的部署恒红

Status: implemented

## Problem

2026-11-24 两个 PR 连续合并（#4 治理门禁面 → #5 发版运维面），两次 `GitOps Deploy (prod)`
**都以同一个原因失败**：

```
CONFLICT (content): Merge conflict in deploy/ponyllm-deployment.yaml
error: could not apply <sha>... deploy(prod): pin image digest sha256:<sha> [skip ci]
```

根因在 `.github/workflows/ci.yml` 的 `Commit digest pin back to main` 步骤：它先 `git commit`
自己的 digest，再 `git fetch origin main && git rebase origin/main`。而 pin 提交改的正是
**同一批 4 行 image 行**——两次 run 各钉自己的 digest，rebase 重放旧 digest 时必然与已被
后来者改写的那 4 行 content 冲突。冲突即 `exit 1`，job 在 pin 阶段就死了，
`Apply / Serialized rollout / Parity Gate / Smoke` 全未执行。

后果不只是红：`main` 上的清单（`deploy/ponyllm-deployment.yaml`）一度钉着 `4db7771` 的 digest，
而集群实际跑的是 `12d53be` 那次的 `8b7fb177…`——**「清单即真相」这条契约当场失真**，
且两次 run 的 commit-back 都没落地，后续任何一次部署都得先猜集群里到底是哪个版本。

同时暴露第二层问题：workflow **没有 concurrency 控制**，两次 main 运行会同时向集群发起
4 role 串行滚动；第一次失败日志里的 `HTTP 503 Service Temporarily Unavailable` 三次重试全中，
与「两个 run 交叉 apply / 交叉滚动」的时间窗吻合，无法区分是外部上游抖动还是自伤。

## Decision

把 pin 从「带历史的提交 + rebase」改成**单点声明 + 超越者让位**，并把 main 部署串行化：

1. **让位语义**：pin 步骤每轮先 `git fetch origin main`，比较 `origin/main` tip 与 `$GITHUB_SHA`。
   不等 → 本次 run 已被后来者超越，`git checkout origin/main -- <manifest>` 把清单交还最新版后
   `exit 0`（**不是失败**：由超越它的那次 run 负责钉自己的 digest）。
2. **基线重钉**：仍是最新 tip 的 run，先 `git checkout origin/main -- <manifest>` 取最新清单，
   再用同一条 `sed`（只认 `sha256:[0-9a-f]*` 占位）钉入自己的 digest，断言恰 4 行。
   重复执行结果一致（幂等），因此重试不需要 rebase。
3. **push 重试而非 rebase**：`git push origin HEAD:main` 失败（并发非快进）→ `git reset --hard HEAD~1`
   丢弃本次提交 → `sleep 5` 重来，最多 3 轮。丢了提交所以**不存在可冲突的对象**。
4. **main 串行**：workflow 顶层加 `concurrency: group: ci-<workflow>-<ref>`，
   `cancel-in-progress` 仅对 `pull_request` 为真——main 上的部署**不可被取消**（半途取消会留下
   已 apply 未验证的集群），PR 上的旧运行则照常被新提交取消，省 runner。
5. **防呆门禁**：新增 `tests/pre-flight/check-ci-digest-pin-semantics.sh`（5 条静态判据：无 rebase /
   有 superseded 判定 / 有 origin-main 基线 / push 可重试 / 有 concurrency 且 cancel 仅 PR）
   + 负样本 `…spec.sh`（P1 + N1–N7，8 条断言），接入 CI `test` job 与既有 prober 契约门并列。
   判据只读 `ci.yml` 文本，不连网不连集群。

## Alternatives considered

- **保留 rebase，冲突时 `git checkout --ours` 了事**（落选）：`--ours` 会保住本 run 的旧 digest，
  把「清单 = 最新」反向覆盖成「清单 = 上一版」，比失败更坏——集群实际镜像会与清单永久错位。
- **把 pin 改成 `[skip ci]` 之外的独立 job / 用 GitHub API 直接改文件**（落选）：API 写路径绕开
  仓库 CAS 与本地门禁，等于给「清单」开一条无审查的写入口；而独立 job 只是把同一个竞态挪个位置。
- **pin 冲突时静默跳过（只打 WARN 不 exit）**（落选）：跳过意味着清单停在旧 digest 却继续
  `apply` + 滚动，制造「部署成功但清单没跟上」的假绿——正是本次要消灭的那类失真。
- **用 `cancel-in-progress: true` 对 main 也生效**（落选）：main 上被取消的 run 可能停在
  「已 apply 未验收」或「已滚动一半」状态，留下无法自动收敛的集群；main 需要的是**串行**不是取消。
- **只在文档里写「别连续合并」**（落选）：靠人纪律替代机械判据，与本仓常载命约 2 冲突；
  且这条纪律下次发版就会被忘掉。

## Consequences

- 连续两次 main push 不再产生 pin 冲突：前者让位、后者钉自己的 digest，清单恒等于最新发布镜像。
- 清单与集群的对应关系恢复单点真值；`check-cluster-image-parity.sh` 的门面语义重新成立。
- 部署在 main 上串行执行，交叉滚动与「长流门禁撞上别人滚动窗口」这类自伤被消除；
  代价是连续合并时后一次部署排队等待（发版节奏上可接受）。
- 门禁落地后，任何人把 pin 步骤改回 rebase / 去掉让位判定 / 去掉 concurrency，CI `test` job 立刻红。
- 2026-11-24 那次 503 的外部归因仍**未定论**：串行化只是排除了自伤这一半，
  上游真抖动的判定需要 `B<alerting>` 那条告警链路（已实现、默认关闭）上线后才可机械区分。
