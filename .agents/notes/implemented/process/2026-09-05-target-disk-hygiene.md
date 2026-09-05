# Agent Note: 收敛 target 中间产物与运行期零散文件

Status: implemented

## Problem

`target/` 膨胀至 89.1GB：`target/debug/deps` 39.3GB（4802 个文件，675 个 `*_tests-*.exe`、696 个 `.pdb`，单个测试 `.pdb` 约 110MB），`target/debug/incremental` 49.4GB（171751 个文件，1277 个哈希目录）。根因有三：cargo 永不回收旧哈希产物（每次改动产生新哈希，旧二进制与增量缓存永久残留）；dev 默认全量调试信息使每个测试 `.pdb` 超百 MB；`.gitignore` 仅覆盖 `target/`，`ponyllm.pid`（stray 运行期文件，已出现在仓库根）与 `ponyllm-serve.log`、覆盖率产物均未覆盖。

## Decision

清理执行 surgical GC 而非全量 `cargo clean`：删除 `target/debug/incremental`（纯缓存）；`target/debug/deps` 内按测试主干分组，每组仅保留最新的 2 个 `*.exe` 及其同名 `.pdb`/`.d`（446 组旧产物，释放 25.6GB）；删除仓库根 stray `ponyllm.pid`。`target/` 现为 14.1GB，当前构建指纹保持命中。

配置收敛两处：`Cargo.toml` 新增 `[profile.dev] debug = 1`（行表级调试信息，保留可用回溯，单个 `.pdb` 体积下降约一个数量级），`incremental = true` 显式固化本地增量意图；`.gitignore` 追加运行期产物（`ponyllm.pid`、`ponyllm-serve.log`）、覆盖率与画像产物（`coverage/`、`lcov.info`、`*.profraw`、`*.profdata`）、`target` 外兜底 `*.pdb` 与 IDE 目录（`.idea/`、`.vscode/`、`*.swp`、`*.swo`）。

清理前已搜消费者：唯一引用 `target/` 的是 `.github/workflows/release.yml` 的 release 三元组产物路径，与本次删除的 `target/debug` 无交集；恢复条件为 `cargo check --workspace --tests`（增量缓存与被删旧哈希按需重建）。

## Alternatives considered

- **全量 `cargo clean`（删整个 `target/`）**：否决——同样释放 75GB，但下次构建变为全量重编（数分钟），而 surgical GC 保留最新指纹，下次构建仍走增量。
- **引入 `cargo-sweep` 做定时 GC**：否决——新增第三方工具依赖与学习成本；cargo 原生行为 + 手动 GC 命令已够用，待堆积复发再评估。
- **`debug = 0`（彻底关闭 dev 调试信息）**：否决——`.pdb` 归零但 `cargo test` 失败时无符号回溯，排查成本上升；`debug = 1` 在体积与可调试性之间取平衡。
- **把门禁接进 `pre-commit`/`pre-push`**：否决——扫描数千文件违背秒级/10秒级门禁预算；本次仅在 `## Consequences` 留可手工执行的 GC 命令，不立法自动门禁。
- **保持现状（只删不防）**：否决——旧哈希按每次构建数百 MB 持续堆积，数日即回弹数十 GB。

## Consequences

- `target/` 89.1GB → 14.1GB；后续 `cargo test` 产物按 `debug = 1` 生成，`.pdb` 不再是百 MB 级。
- 旧哈希仍会随日常开发缓慢堆积，复发时执行：全量核选项 `cargo clean`，或 surgical（每类测试留新删旧，连带同名 `.pdb`/`.d`）。
- 验证（均为非零退出命令）：
  - `git check-ignore ponyllm.pid ponyllm-serve.log`
  - `cargo check --workspace --tests`
  - `bash .agents/skills/write-adr/verify-note.sh .agents/notes/implemented/process/2026-09-05-target-disk-hygiene.md`
