# Agent Note: Dockerfile 多阶段构建消除 GLIBC 漂移

Status: implemented

## Problem

生产发布（2026-09-28，v0.2.45）把本地 Ubuntu 24.04（glibc 2.39）上 `cargo build --release`
的产物打进 `debian:bookworm-slim`（glibc 2.36）运行镜像后，新 Pod 启动即崩溃并
CrashLoopBackOff：`/lib/x86_64-linux-gnu/libc.so.6: version 'GLIBC_2.39' not found`。

根因：旧 Dockerfile 是单阶段"COPY 预构建产物"，构建机 glibc 版本决定了二进制的
最低 glibc 需求（rustc 链接的是构建机 glibc 的符号版本，Ubuntu 24.04 = 2.39 >
bookworm 的 2.36）。任何在较新的 Ubuntu/Debian 上手动或 CI（ubuntu-latest 同为
glibc 2.39）构建的产物都会在容器内无法启动。滚动更新（maxSurge=1/maxUnavailable=0）
保证了旧 Pod 全程在线、服务未下线，但新版本无法上线。

## Decision

`deploy/Dockerfile` 改为**多阶段构建**：

- 构建阶段 `rust:1.91-bookworm`（glibc 2.36）：`WORKDIR /build`，
  `COPY Cargo.toml Cargo.lock + crates/` 后 `cargo build --release --bin ponyllm`，
  带 BuildKit cache mount（`/build/target` 与 `/usr/local/cargo/registry`）加速重编；
- 运行阶段 `debian:bookworm-slim@sha256:60eac7...`（原锁定哈希不变）：
  `COPY --from=builder /build/target/release/ponyllm`，`COPY web/dist`（web 静态产物
  仍由外部构建，保持现状）；
- 新增 `.dockerignore`（`.git`、`target`、`node_modules`），避免把宿主编译产物与
  依赖树拷进构建上下文，保证容器内产物**只**由 bookworm 工具链产生。

产物 glibc 需求（2.36）与运行镜像严格一致，手动本地发布与 GitHub CI
（`docker/build-push-action`，context: .）从此走同一条自洽构建，不再受构建机影响。

## Alternatives considered

1. **保持单阶段、在 rust:1.91-bookworm 容器内手动构建后再 COPY**——只修这次，
   CI 的 GHCR 镜像仍会产出同样崩溃的产物，问题会复发，且多一次手工步骤。
2. **改用 musl 静态链接（`x86_64-unknown-linux-musl`）**——彻底免 glibc 依赖，
   但 rustup 需额外安装 musl target、`openssl`/`ring` 等依赖的静态编译配置复杂，
   引入新的编译面，收益低于把构建搬进 bookworm。
3. **运行镜像升级到 glibc 2.39 的系统（如 debian:testing）**——放弃已锁定的
   bookworm-slim 安全哈希，且把"运行环境追着构建机走"的依赖方向搞反了。

## Consequences

- 本地多阶段构建首次需拉取 `rust:1.91-bookworm` 并全量编译（数分钟）；BuildKit
  cache mount 使后续增量构建明显提速。
- CI `build-and-push-image` job 不再需要前置的宿主 `cargo build --release`（原
  job 的编译步骤可精简，但保留无碍、仅冗余）——后续可清理。
- 回滚语义不变：生产仍以 digest 引用镜像。