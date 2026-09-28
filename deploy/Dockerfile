# syntax=docker/dockerfile:1
# 多阶段可复现构建（2026-09-28 修复 GLIBC 漂移）：
# Rust 产物在 rust:1.91-bookworm（glibc 2.36）内编译，与运行镜像
# debian:bookworm-slim（glibc 2.36）严格匹配；宿主较新 glibc 编译的
# 产物会因 GLIBC_2.39 not found 无法在容器内启动（生产发布踩坑实证）。
# 运行阶段基础镜像仍锁定官方 debian:bookworm-slim 可复现构建安全哈希。
FROM rust:1.91-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release --bin ponyllm

FROM debian:bookworm-slim@sha256:60eac759739651111db372c07be67863818726f754804b8707c90979bda511df

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/ponyllm /usr/local/bin/ponyllm
COPY web/dist /opt/ponyllm/web/dist
RUN groupadd -r -g 10001 ponyllm && useradd -r -u 10001 -g ponyllm -s /usr/sbin/nologin ponyllm \
    && chown -R 10001:10001 /usr/local/bin/ponyllm /opt/ponyllm \
    && chmod +x /usr/local/bin/ponyllm

USER 10001:10001

EXPOSE 8080
ENTRYPOINT ["ponyllm"]
CMD ["serve", "--bind", "0.0.0.0:8080"]
