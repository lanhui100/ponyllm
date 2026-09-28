# 基础镜像已锁定官方 debian:bookworm-slim 可复现构建安全哈希
FROM debian:bookworm-slim@sha256:60eac759739651111db372c07be67863818726f754804b8707c90979bda511df

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY target/release/ponyllm /usr/local/bin/ponyllm
COPY web/dist /opt/ponyllm/web/dist
RUN groupadd -r -g 10001 ponyllm && useradd -r -u 10001 -g ponyllm -s /usr/sbin/nologin ponyllm \
    && chown -R 10001:10001 /usr/local/bin/ponyllm /opt/ponyllm \
    && chmod +x /usr/local/bin/ponyllm

USER 10001:10001

EXPOSE 8080
ENTRYPOINT ["ponyllm"]
CMD ["serve", "--bind", "0.0.0.0:8080"]
