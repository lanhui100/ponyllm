-- Migration: 0001_telemetry_hourly_cluster.sql
-- 全集群遥测小时聚合表（无时序插件依赖，利用原生 B-Tree 索引与原子 UPSERT）

CREATE TABLE IF NOT EXISTS telemetry_hourly_cluster (
    bucket_hour_ms BIGINT NOT NULL,
    provider VARCHAR(64) NOT NULL DEFAULT '',
    model VARCHAR(128) NOT NULL DEFAULT '',
    total_requests BIGINT NOT NULL DEFAULT 0,
    failed_requests BIGINT NOT NULL DEFAULT 0,
    prompt_tokens BIGINT NOT NULL DEFAULT 0,
    completion_tokens BIGINT NOT NULL DEFAULT 0,
    cached_tokens BIGINT NOT NULL DEFAULT 0,
    latency_sum_ms DOUBLE PRECISION NOT NULL DEFAULT 0,
    latency_count BIGINT NOT NULL DEFAULT 0,
    ttft_sum_ms DOUBLE PRECISION NOT NULL DEFAULT 0,
    ttft_count BIGINT NOT NULL DEFAULT 0,
    tps_sum_milli BIGINT NOT NULL DEFAULT 0,
    tps_count BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    PRIMARY KEY (bucket_hour_ms, provider, model)
);

CREATE INDEX IF NOT EXISTS idx_telemetry_hourly_cluster_time ON telemetry_hourly_cluster(bucket_hour_ms);

GRANT ALL PRIVILEGES ON TABLE telemetry_hourly_cluster TO ponyllm_lock;
