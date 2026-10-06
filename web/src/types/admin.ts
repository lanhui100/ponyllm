// Types aligned with openapi.json schemas for Admin API

export interface OverviewView {
  version: string;
  bind: string;
  auth_mode: string;
  providers: number;
  keys: number;
  keys_active: number;
  strategy: string;
  hot_reload_ms: number;
  admin_write_enabled: boolean;
  config_version: number;
  /** Auth compatibility mode echo (`legacy-only` | `dual` | `strict`). */
  auth_compat?: string;
}

export interface ProviderView {
  name: string;
  base_url: string;
  default_model: string;
  strategy: string;
  billing_mode: string;
  input_price: number;
  cached_price: number;
  output_price: number;
  models: number;
  default_protocol?: string | null;
  chat_url?: string | null;
  responses_url?: string | null;
  messages_url?: string | null;
  /** Egress pool (出口池轮询): `direct` 或代理 URL 列表；null/缺省 = 单 proxy 语义。 */
  egress_pool?: string[] | null;
  /** Egress rotation strategy: `round_robin` | `priority`. */
  egress_strategy?: string;
}

/** 短窗频率限额（M3 统一额度计量内核）：per-key 60s 滑动窗口 + 并发。
    snake_case 对齐 Admin API `/api/admin/models/{name}` 的 `rate_limits` 字段；
    `null` 表示未配置（继承 provider 级默认/上游不限）。 */
export interface RateLimits {
  /** 每分钟请求数上限（requests per minute）。 */
  rpm?: number | null;
  /** 每分钟 token 上限（tokens per minute）。 */
  tpm?: number | null;
  /** 滑动窗口秒数，默认 60。 */
  window_secs?: number | null;
  /** 同 key 并发在途上限。 */
  concurrency?: number | null;
  /** 缓存 token 是否计入 TPM（按上游口径，默认 false）。 */
  count_cached?: boolean | null;
}

export type PricingMode = 'uniform' | 'peak_valley';

export interface PricingPeriod {
  name?: string;
  start_time: string;
  end_time: string;
  input_price: number;
  cached_price: number;
  output_price: number;
  include_weekends?: boolean;
}

export interface ModelView {
  provider?: string;
  name: string;
  tier: string;
  context_window: string;
  input_types?: string[];
  output_types?: string[];
  thinking_default: string;
  thinking_max: string;
  protocol?: string | null;
  base_url?: string | null;
  /** Per-model outbound proxy override (`Some` = 该模型经代理拨上游；`None` = 直连/继承服务商). */
  proxy?: string | null;
  input_price?: number | null;
  cached_price?: number | null;
  output_price?: number | null;
  pricing_mode?: PricingMode | null;
  pricing_periods?: PricingPeriod[] | null;
  temperature?: number | null;
  top_p?: number | null;
  display_name?: string | null;
  /** Explicit routing preference for this model under this provider: larger = tried first among same-named models. */
  priority?: number | null;
  /** 短窗频率限额（RPM/TPM/窗口/并发），见 [`RateLimits`]。 */
  rate_limits?: RateLimits | null;
}

export interface UpstreamModelItem {
  id: string;
}

export interface UpstreamModelsView {
  provider: string;
  source: string;
  models: UpstreamModelItem[];
}

export interface KeyView {
  id: string;
  provider: string;
  masked_key: string;
  priority: number;
  weight: number;
  state: string;
  /** Seconds left in the current cooldown (only while `cooling_down`). */
  cooldown_remaining_secs?: number | null;
  /** RFC 3339 UTC instant the key is expected to recover after a cooldown. */
  cooldown_reset_at?: string | null;
  /** Why the key is cooling down, while it is: `rate_limit | quota | server | eligibility`. */
  cooldown_reason?: string | null;
  /** Human-readable reason for a hard non-active state (eligibility freeze / permanent disable). */
  error_message?: string | null;
  /** Reason why the key was permanently disabled, if state is `disabled`. */
  disabled_reason?: string | null;
  usage?: KeyCapacityEstimate | null;
  /** 出口池视图（provider 级，同 provider 每行重复）：每出口 state/cooldown_reset_at。 */
  egress?: QuotaEgressView[] | null;
}

/** 管理面出口池条目视图（契约 C7）：index=池内位置、entry="direct"或代理 URL、
    state="active"|"cooling"、cooldown_reset_at=RFC3339（冷却中）。 */
export interface QuotaEgressView {
  index: number;
  entry: string;
  state: 'active' | 'cooling' | string;
  cooldown_reset_at?: string | null;
}

export interface AntigravityQuotaItemView {
  model_id: string;
  remaining_fraction: number;
  reset_time?: string | null;
  reset_time_beijing?: string | null;
  time_until_reset?: string | null;
}

export interface AntigravityQuotaBucketView {
  bucket_id: string;
  window: string;
  remaining_fraction: number;
  reset_time?: string | null;
  reset_time_beijing?: string | null;
  time_until_reset?: string | null;
  display_name?: string | null;
  description?: string | null;
}

export interface AntigravityQuotaGroupView {
  display_name: string;
  description?: string | null;
  buckets: AntigravityQuotaBucketView[];
}

export interface WindowUsage {
  prompt_tokens: number;
  completion_tokens: number;
  cached_tokens: number;
  total_tokens: number;
  requests: number;
}

export interface CycleStats {
  count: number;
  prompt_tokens?: number;
  completion_tokens?: number;
  cached_tokens?: number;
  total_tokens: number;
  requests?: number;
  avg_tokens: number;
}

export interface KeyCapacityEstimate {
  window_5h: WindowUsage;
  window_weekly: WindowUsage;
  window_monthly?: WindowUsage | null;
  completed_5h_stats?: CycleStats | null;
  completed_weekly_stats?: CycleStats | null;
  estimated_capacity_5h?: number | null;
  estimated_tokens_remaining_5h?: number | null;
  estimated_capacity_weekly?: number | null;
  account_tier: 'pro' | 'standard' | 'free' | 'calibrating' | 'unknown' | string;
  confidence: number;
  calibration_status?: 'benchmarked' | 'estimated' | 'calibrating' | string;
}

/** 池级跨账号跨周期持久化累计基准（一个窗口档位）。 */
export interface QuotaBenchmarkKindView {
  /** 已累计的 (账号 × 已闭合对齐周期) 观测数。 */
  observations: number;
  /** 每观测平均 token（= 累计总量 / 观测数）。 */
  avg_tokens: number;
  prompt_tokens: number;
  completion_tokens: number;
  cached_tokens: number;
  total_tokens: number;
  requests: number;
  /** 已累计的打满（完整周期）实测轮数。 */
  completed_cycles: number;
  /** 每轮打满实测平均 token。 */
  avg_completed_tokens: number;
  completed_prompt_tokens: number;
  completed_completion_tokens: number;
  completed_cached_tokens: number;
  completed_total_tokens: number;
  completed_requests: number;
  first_observation_ms: number;
  last_observation_ms: number;
}

/** `GET /api/admin/quota/benchmark` —— 持久化累计基准（跨发布/账号增删不归零）。 */
export interface QuotaCycleBenchmarkView {
  persisted_at_ms: number;
  kind_5h: QuotaBenchmarkKindView;
  kind_weekly: QuotaBenchmarkKindView;
  kind_monthly: QuotaBenchmarkKindView;
}

export interface KeyTestView {
  success: boolean;
  latency_ms: number;
  message: string;
  http_status?: number | null;
  error_code?: string | null;
  quota?: AntigravityQuotaItemView[] | null;
  quota_groups?: AntigravityQuotaGroupView[] | null;
  usage?: KeyCapacityEstimate | null;
}

export interface StrategyView {
  strategy: string;
  config_version: number;
}

export interface ServiceStatusView {
  uptime_seconds: number;
  bind: string;
  web_enabled: boolean;
  admin_write_enabled: boolean;
  config_version: number;
}

export interface CreateProviderPayload {
  name: string;
  base_url: string;
  default_model?: string;
  strategy?: string;
  billing_mode?: string;
  input_price?: number;
  cached_price?: number;
  output_price?: number;
  chat_url?: string | null;
  messages_url?: string | null;
  responses_url?: string | null;
  proxy?: string | null;
  default_protocol?: string | null;
}

export interface UpdateProviderPayload {
  base_url?: string | null;
  default_model?: string | null;
  strategy?: string | null;
  default_protocol?: string | null;
  chat_url?: string | null;
  responses_url?: string | null;
  messages_url?: string | null;
  proxy?: string | null;
  /** 出口池条目；显式空数组 = 清空并回退单 proxy 语义。 */
  egress_pool?: string[] | null;
  /** 出口轮询策略：`round_robin` | `priority`。 */
  egress_strategy?: string | null;
}

export interface CreateModelPayload {
  name: string;
  provider: string;
  tier?: string | null;
  context_window?: string | null;
  max_output?: string | null;
  input_types?: string[] | null;
  output_types?: string[] | null;
  protocol?: string | null;
  base_url?: string | null;
  proxy?: string | null;
  thinking_default?: string | null;
  thinking_max?: string | null;
  input_price?: number | null;
  cached_price?: number | null;
  output_price?: number | null;
  pricing_mode?: PricingMode | null;
  pricing_periods?: PricingPeriod[] | null;
  temperature?: number | null;
  top_p?: number | null;
  display_name?: string | null;
  /** Explicit routing preference for this model under this provider (larger = tried first). */
  priority?: number | null;
  /** 短窗频率限额（RPM/TPM/窗口/并发）。 */
  rate_limits?: RateLimits | null;
}

export interface UpdateModelPayload {
  provider?: string | null;
  tier?: string | null;
  context_window?: string | null;
  max_output?: string | null;
  input_types?: string[] | null;
  output_types?: string[] | null;
  protocol?: string | null;
  base_url?: string | null;
  proxy?: string | null;
  thinking_default?: string | null;
  thinking_max?: string | null;
  input_price?: number | null;
  cached_price?: number | null;
  output_price?: number | null;
  pricing_mode?: PricingMode | null;
  pricing_periods?: PricingPeriod[] | null;
  temperature?: number | null;
  top_p?: number | null;
  display_name?: string | null;
  /** Explicit routing preference for this model under this provider (larger = tried first). */
  priority?: number | null;
  /** 短窗频率限额（RPM/TPM/窗口/并发），null 时服务商按该字段是否配置决定是否覆盖 heredoc provider 默认。 */
  rate_limits?: RateLimits | null;
}

export interface CreateKeyPayload {
  id: string;
  provider: string;
  api_key: string;
  priority?: number;
  weight?: number;
}

export interface UpdateKeyPayload {
  provider?: string;
  priority?: number;
  weight?: number;
}

export interface CreateKeyResponse {
  id: string;
  provider: string;
  api_key: string;
  priority: number;
  weight: number;
  state: string;
  config_version: number;
}

export interface PutStrategyPayload {
  strategy: string;
}

/**
 * Scoped gateway credential (task-28). Read projection NEVER carries the
 * plaintext, the salt, or the hash — only `prefix` + `last4` identification.
 */
export interface GatewayKeyView {
  id: string;
  scope: string;
  prefix: string;
  last4: string;
  revoked: boolean;
  expires_at?: number | null;
  config_version: number;
}

export interface IssueGatewayKeyPayload {
  id: string;
  scope: string;
  expires_at?: number | null;
}

/** One-time issuance response: `api_key` is the ONLY place the plaintext exists. */
export interface IssueGatewayKeyResponse {
  id: string;
  scope: string;
  api_key: string;
  expires_at?: number | null;
  config_version: number;
}

export interface AdminConflictError {
  isConflict: true;
  status: 412;
  message: string;
}

export interface AntigravityAuthUrlView {
  auth_url: string;
  redirect_uri: string;
  state: string;
}

export interface AuthorizeAntigravityPayload {
  code_or_url: string;
  provider?: string | null;
  id?: string | null;
  priority?: number;
  weight?: number;
  redirect_uri?: string | null;
  state?: string | null;
  proxy?: string | null;
}

export interface AuthorizeAntigravityResponse {
  provider: string;
  id: string;
  email?: string | null;
  config_version: number;
  quota?: AntigravityQuotaItemView[] | null;
  quota_groups?: AntigravityQuotaGroupView[] | null;
}

export interface ProxyStatusView {
  available: boolean;
  proxy_url?: string | null;
  proxy_type: 'pproxy' | 'system' | 'custom' | 'none' | string;
  description: string;
  latency_ms?: number | null;
  hint: string;
}

export interface AntigravityPendingView {
  state: string;
  ready: boolean;
  code?: string | null;
  error?: string | null;
}
