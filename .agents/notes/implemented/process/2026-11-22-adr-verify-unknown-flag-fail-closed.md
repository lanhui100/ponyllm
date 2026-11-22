# Agent Note: adr-verify 对未知选项 fail-closed（修复治理门负样本失配）

Status: implemented

## Problem

`.meta/gates/pre-commit.spec.sh` 的负样本契约要求：给 `adr-verify.sh` 传入不存在的选项时必须以非零退出，
证明治理门会拒绝非法输入。但 `adr-verify.sh` 的带参模式把所有非 `.agents/notes/` 路径（包括
`--non-existent-illegal-flag-test-fail`）都当作「宽参」跳过，最终走到 `skip "无暂存 ADR 变更"` 并以 0 退出。
结果是 `pre-commit.spec.sh` 在 HEAD 上就失败（`gate did not reject invalid input`），任何本地提交都被
pre-commit 拦下；同时这是一个真实的 fail-open：把选项拼错成 `--staged`、`--all` 之类时，门会静默按
「无暂存 ADR」放行，而不是告诉调用者参数无效。

## Decision

在 `.meta/gates/adr-verify.sh` 的带参解析循环最前面加一条判据：任何以 `-` 开头的参数一律 `die`
（exit 1，文案点名「未知参数」与用法），不再落入「忽略非 ADR 路径」。其余语义保持不变：

- 无参模式仍只校验暂存区 ADR；
- 带参模式下，非 `.agents/notes/` 的普通路径仍按既有约定 `note` 后跳过（避免误伤调用者传宽路径集）；
- ADR 内容判据仍只由 `verify-note.sh` 判（P9：一份判据一份实现）；
- `pre-commit.spec.sh` 不修改——它本来就是这条契约的负样本，修的是门，不是测试。

收据（可复跑）：`bash .meta/gates/adr-verify.sh --non-existent-illegal-flag-test-fail` → exit 1，
stderr 含「未知参数」；`bash .meta/gates/adr-verify.sh` → exit 0；`bash .meta/gates/pre-commit.spec.sh`
→ exit 0（`All negative spec tests passed`）；`bash .meta/gates/adr-verify.spec.sh` → exit 0（38 asserts）。

范围说明：工作区里 `adr-verify.sh` 另有一批**未暂存**的既有治理升级（U3 删除入册、A6 缺失/空壳
fail-closed 等），不属于本次修复。本次提交只把「HEAD 版 + 上述最小修复」的 blob 入索引，
不改写、不纳入那些未暂存改动；工作区文件保留原改动并同步加上了同一条未知选项判据，保证本地门也能通过。

## Alternatives considered

- **改 `pre-commit.spec.sh`，接受 exit 0**：能让测试变绿，但等于把「未知选项静默放行」写成契约，
  是往 fail-open 方向改门禁，故否决。
- **带参模式拒绝一切非 `.agents/notes/` 路径**：更严格，但会误伤调用者传宽路径集的既有用法，
  且与本次要修的「未知选项」不是同一类风险；故只拒绝 `-` 开头的参数。
- **新增 `--` 分隔符或显式子命令**：当前门没有任何合法选项，引入 CLI 语法只会增加维护面；
  等出现真实选项需求再设计，故不做。
- **把工作区既有治理升级一并提交**：那些改动范围大、与本次缺陷无因果关系，且未经过本次验证；
  静默纳入会把「修一个参数判据」变成「替别人收口一次框架升级」，故不纳入。

## Consequences

- 本地 `pre-commit` 的 `pre-commit.spec.sh` 恢复为可通过；拼错的选项不再静默放行。
- `adr-verify.sh` 的公开调用契约多一条：以 `-` 开头的参数会被拒绝；无参/普通路径调用不受影响。
- 残留：工作区仍有未暂存的治理升级改动；它们与本次提交的 HEAD+fix blob 不同，后续提交时需单独评审。
