FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

COPY target/release/ponyllm /usr/local/bin/ponyllm
COPY web/dist /opt/ponyllm/web/dist
RUN chmod +x /usr/local/bin/ponyllm

EXPOSE 8080
ENTRYPOINT ["ponyllm"]
CMD ["serve", "--bind", "0.0.0.0:8080"]
