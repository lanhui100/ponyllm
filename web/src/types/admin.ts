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
  input_price?: number | null;
  cached_price?: number | null;
  output_price?: number | null;
  pricing_mode?: PricingMode | null;
  pricing_periods?: PricingPeriod[] | null;
  temperature?: number | null;
  top_p?: number | null;
  display_name?: string | null;
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

export interface KeyTestView {
  success: boolean;
  latency_ms: number;
  message: string;
  http_status?: number | null;
  error_code?: string | null;
  quota?: AntigravityQuotaItemView[] | null;
  quota_groups?: AntigravityQuotaGroupView[] | null;
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
}

export interface CreateKeyPayload {
  id: string;
  provider: string;
  api_key: string;
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
