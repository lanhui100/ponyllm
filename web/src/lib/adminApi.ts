import { alova } from './alova';
import type {
  OverviewView,
  ServiceStatusView,
  ProviderView,
  ModelView,
  KeyView,
  KeyTestView,
  AutoModelsView,
  PutAutoModelsPayload,
  CreateProviderPayload,
  UpdateProviderPayload,
  CreateModelPayload,
  UpdateModelPayload,
  CreateKeyPayload,
  UpdateKeyPayload,
  CreateKeyResponse,
  AntigravityAuthUrlView,
  AntigravityPendingView,
  AuthorizeAntigravityPayload,
  AuthorizeAntigravityResponse,
  ProxyStatusView,
  UpstreamModelsView,
  GatewayKeyView,
  IssueGatewayKeyPayload,
  IssueGatewayKeyResponse,
  QuotaCycleBenchmarkView,
} from '../types/admin';

export function ifMatchHeaders(version?: number | string): Record<string, string> {
  if (version === undefined || version === null || version === '') {
    return {};
  }
  return { 'If-Match': String(version) };
}

export const adminApi = {
  // Read APIs
  getOverview() {
    return alova.Get<OverviewView>('/api/admin/overview');
  },

  getServiceStatus() {
    return alova.Get<ServiceStatusView>('/api/admin/service/status');
  },

  getProviders() {
    return alova.Get<ProviderView[]>('/api/admin/providers');
  },

  getModels() {
    return alova.Get<ModelView[]>('/api/admin/models');
  },

  getProviderModels(providerName: string) {
    return alova.Get<ModelView[]>(`/api/admin/providers/${encodeURIComponent(providerName)}/models`);
  },

  getUpstreamModels(providerName: string) {
    return alova.Get<UpstreamModelsView>(
      `/api/admin/providers/${encodeURIComponent(providerName)}/upstream-models`,
    );
  },

  getKeys() {
    return alova.Get<KeyView[]>('/api/admin/keys');
  },

  getAutoModels() {
    return alova.Get<AutoModelsView>('/api/admin/auto-models');
  },

  updateAutoModels(payload: PutAutoModelsPayload, ifMatchVersion?: number | string) {
    return alova.Put<AutoModelsView>('/api/admin/auto-models', payload, {
      headers: ifMatchHeaders(ifMatchVersion),
    });
  },

  /** 池级跨账号跨周期持久化累计基准（只读，直接读快照归档）。 */
  getQuotaBenchmark() {
    return alova.Get<QuotaCycleBenchmarkView>('/api/admin/quota/benchmark');
  },

  /** Scoped gateway credentials (task-28). AdminRead: inference callers get 403. */
  getGatewayKeys() {
    return alova.Get<GatewayKeyView[]>('/api/admin/gateway-keys');
  },

  // Write APIs (With optional If-Match optimistic concurrency headers)
  createProvider(payload: CreateProviderPayload, ifMatchVersion?: number | string) {
    return alova.Post<ProviderView>('/api/admin/providers', payload, {
      headers: ifMatchHeaders(ifMatchVersion),
    });
  },

  updateProvider(name: string, payload: UpdateProviderPayload, ifMatchVersion?: number | string) {
    return alova.Put<ProviderView>(`/api/admin/providers/${encodeURIComponent(name)}`, payload, {
      headers: ifMatchHeaders(ifMatchVersion),
    });
  },

  deleteProvider(name: string, ifMatchVersion?: number | string) {
    return alova.Delete<{ ok: boolean; config_version: number }>(
      `/api/admin/providers/${encodeURIComponent(name)}`,
      undefined,
      {
        headers: ifMatchHeaders(ifMatchVersion),
      },
    );
  },

  createModel(payload: CreateModelPayload, ifMatchVersion?: number | string) {
    return alova.Post<ModelView>('/api/admin/models', payload, {
      headers: ifMatchHeaders(ifMatchVersion),
    });
  },

  updateModel(name: string, payload: UpdateModelPayload, ifMatchVersion?: number | string) {
    return alova.Put<ModelView>(`/api/admin/models/${encodeURIComponent(name)}`, payload, {
      headers: ifMatchHeaders(ifMatchVersion),
    });
  },

  deleteModel(name: string, ifMatchVersion?: number | string, provider?: string) {
    const qs = provider ? `?provider=${encodeURIComponent(provider)}` : '';
    return alova.Delete<{ ok: boolean; config_version: number }>(
      `/api/admin/models/${encodeURIComponent(name)}${qs}`,
      undefined,
      {
        headers: ifMatchHeaders(ifMatchVersion),
      },
    );
  },

  createKey(payload: CreateKeyPayload, ifMatchVersion?: number | string) {
    return alova.Post<CreateKeyResponse>('/api/admin/keys', payload, {
      headers: ifMatchHeaders(ifMatchVersion),
    });
  },

  updateKey(id: string, payload: UpdateKeyPayload, ifMatchVersion?: number | string) {
    return alova.Put<KeyView>(`/api/admin/keys/${encodeURIComponent(id)}`, payload, {
      headers: ifMatchHeaders(ifMatchVersion),
    });
  },

  deleteKey(id: string, ifMatchVersion?: number | string) {
    return alova.Delete<{ ok: boolean; config_version: number }>(
      `/api/admin/keys/${encodeURIComponent(id)}`,
      undefined,
      {
        headers: ifMatchHeaders(ifMatchVersion),
      },
    );
  },

  testKey(id: string) {
    return alova.Post<KeyTestView>(`/api/admin/keys/${encodeURIComponent(id)}/test`);
  },

  /** Issue a scoped gateway key: plaintext returned ONCE (no-store server side). */
  issueGatewayKey(payload: IssueGatewayKeyPayload, ifMatchVersion?: number | string) {
    return alova.Post<IssueGatewayKeyResponse>('/api/admin/gateway-keys', payload, {
      headers: ifMatchHeaders(ifMatchVersion),
    });
  },

  /** Delete a scoped gateway key (hard delete since 2026-09-21): the entry is
      removed from disk + memory and no record of it remains. Unknown id → 404. */
  revokeGatewayKey(id: string, ifMatchVersion?: number | string) {
    return alova.Post<GatewayKeyView>(
      `/api/admin/gateway-keys/${encodeURIComponent(id)}/revoke`,
      {},
      { headers: ifMatchHeaders(ifMatchVersion) },
    );
  },

  getAntigravityAuthUrl(redirectUri?: string, state?: string) {
    const params = new URLSearchParams();
    if (redirectUri) params.set('redirect_uri', redirectUri);
    if (state) params.set('state', state);
    const qs = params.toString() ? `?${params.toString()}` : '';
    return alova.Get<AntigravityAuthUrlView>(`/api/admin/oauth/antigravity/auth-url${qs}`);
  },

  getAntigravityPending(state: string) {
    return alova.Get<AntigravityPendingView>(`/api/admin/oauth/antigravity/pending?state=${encodeURIComponent(state)}`);
  },

  getProxyStatus() {
    return alova.Get<ProxyStatusView>('/api/admin/proxy/status');
  },

  authorizeAntigravity(payload: AuthorizeAntigravityPayload) {
    return alova.Post<AuthorizeAntigravityResponse>('/api/admin/oauth/antigravity/authorize', payload);
  },
};
