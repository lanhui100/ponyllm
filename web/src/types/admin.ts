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

export interface ModelView {
  provider?: string;
  name: string;
  tier: string;
  context_window: string;
  thinking_default: string;
  thinking_max: string;
  protocol?: string | null;
}

export interface KeyView {
  id: string;
  provider: string;
  masked_key: string;
  priority: number;
  weight: number;
  state: string;
}

export interface KeyTestView {
  success: boolean;
  latency_ms: number;
  message: string;
  http_status?: number | null;
  error_code?: string | null;
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
  protocol?: string | null;
  proxy?: string | null;
  thinking_default?: string | null;
  thinking_max?: string | null;
}

export interface UpdateModelPayload {
  provider?: string | null;
  tier?: string | null;
  context_window?: string | null;
  max_output?: string | null;
  protocol?: string | null;
  proxy?: string | null;
  thinking_default?: string | null;
  thinking_max?: string | null;
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
