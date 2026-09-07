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
}

export interface StreamTelemetrySnapshot {
  global: StreamFlowSummary;
  providers: Record<string, ProviderFlowSnapshot>;
  dropped: number;
}

export interface HealthStatus {
  status: 'ok' | 'down' | 'degraded';
  version?: string;
}
