export interface StreamFlowDetail {
  ttft_ms?: number;
  ttlb_ms?: number;
  chunks?: number;
  bytes?: number;
  max_gap_ms?: number;
  stall_count?: number;
  tps?: number;
  tpot_p50_ms?: number;
  tpot_p95_ms?: number;
}

export interface RecordedFrame {
  request_id: string;
  timestamp: string;
  endpoint: string;
  provider?: string;
  key_id: string;
  sanitized_key: string;
  attempt?: number;
  status_code?: number;
  latency_ms: number;
  error?: string;
  request_snippet?: string;
  response_snippet?: string;
  stream_flow?: StreamFlowDetail;
}

export interface StreamFlowSummary {
  stream_count: number;
  avg_ttft_ms?: number;
  avg_ttlb_ms?: number;
  avg_chunks?: number;
  total_stalls: number;
  max_gap_ms?: number;
  avg_tps?: number;
  total_bytes: number;
  total_chunks: number;
}

export interface MetricsSummary {
  total_requests: number;
  successful_requests: number;
  failed_requests: number;
  total_failover: number;
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
  stream: StreamFlowSummary;
}

export interface ProviderFlowSnapshot {
  provider: string;
  stream_count: number;
  avg_ttft_ms?: number;
  avg_tps?: number;
  error_count: number;
  status: 'healthy' | 'degraded' | 'unhealthy';
  uptime_bars?: ConnectivityBarSeries;
  total_tokens?: number;
}

export interface StreamTelemetrySnapshot {
  global: StreamFlowSummary;
  providers: Record<string, ProviderFlowSnapshot>;
  gateway_uptime_bars?: ConnectivityBarSeries;
  dropped: number;
}

export interface HealthStatus {
  status: 'ok' | 'down' | 'degraded';
  version?: string;
}

export type ConnectivityStatus = 'ok' | 'degraded' | 'down' | 'empty';

export interface ConnectivitySlot {
  timestamp_ms: number;
  latency_ms?: number;
  tps?: number;
  status: ConnectivityStatus;
}

export interface ConnectivityBarSeries {
  slots: ConnectivitySlot[];
  latest_latency_ms?: number;
}

export interface MetricBucket {
  timestamp_ms: number;
  qps: number;
  token_throughput: number;
  total_tokens: number;
  prompt_tokens: number;
  completion_tokens: number;
  avg_latency_ms: number;
  error_rate: number;
  total_requests: number;
  failed_requests: number;
  tokens_by_provider: Record<string, number>;
  tokens_by_model: Record<string, number>;
}

export interface TimeseriesHistoryResponse {
  range: string;
  points: MetricBucket[];
  total_requests: number;
  total_tokens: number;
  provider_tokens: Record<string, number>;
  model_tokens: Record<string, number>;
}

