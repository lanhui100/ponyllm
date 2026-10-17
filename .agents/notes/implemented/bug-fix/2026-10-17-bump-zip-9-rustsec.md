# Agent Note: 升级 zip 依赖到 9.0.4 以清除 RUSTSEC-2026-0367/0368/0369

Status: implemented

## Problem

2026-10-17 生产发布 CI 的 `Dependency audits` 步骤失败：`cargo audit` 报告 `zip v2.4.2`（`ponyllm-cli` 的直接依赖，用于 self-upgrade 的 zip 解包）命中 3 条新披露漏洞——

- RUSTSEC-2026-0369（severity 9.1 critical）：`ZipArchive::extract()` 经 symlink 逃逸目标目录；
- RUSTSEC-2026-0368（high）：attacker-controlled symlink target 落在目的目录外；
- RUSTSEC-2026-0367（high）：ZIP 解包保留 SUID/SGID 位，可致提权。

审计门禁失败导致 `build-and-push-image` 与 `GitOps Deploy (prod)` 被 skip，B003 生产发布被阻断。该漏洞与 B003 变更无关，是依赖侧新披露。

## Decision

将 workspace 依赖 `zip` 从 `2.2`（锁定 2.4.2）升级到 `9.0.4`（advisory 要求 `>=9.0.0`），保持 `default-features = false, features = ["deflate"]` 不变。`ponyllm-cli/src/upgrade.rs:114` 的 `ZipArchive::new / by_index / enclosed_name / read` API 在 9.0.4 下零改动可编译、`cargo test -p ponyllm-cli` 通过。

## Alternatives considered

- **在 cargo-audit 中 ignore 这三条 advisory**：等同把已知 critical/high 漏洞带进生产，且审计门禁失去意义；拒绝。
- **仅升级 zip 到 2.x 最新**：advisory 明确修复版本为 `>=9.0.0`，2.x 无修复版本；拒绝。
- **自己实现 zip 解包或改走 tar.gz**：扩大变更面、重新引入未验证的解包路径；`enclosed_name()` 已提供路径逃逸防护，保持现有实现+升级版本是最小闭包。
- **推迟发布、等上游依赖迁移**：生产发布被阻断，且修复路径已具备（API 兼容），无等待收益。

## Consequences

- `cargo audit` 的 3 条 vulnerability 清除（unmaintained 类 warning 维持不变）。
- CI 新增依赖：`zip 9.0.4` 的传递依赖（`nix`、`typed-path`、`zlib-rs` 等）；后续由同一 audit 门禁持续覆盖。
- self-upgrade 的 zip 解包行为保持（`enclosed_name()` 路径校验不变）；`cargo test -p ponyllm-cli` exit 0。
- 版本升级与 RUSTSEC 编号写入 commit message，便于审计追溯。
