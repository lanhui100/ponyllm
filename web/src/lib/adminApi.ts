import { alova } from './alova';
import type {
  OverviewView,
  ServiceStatusView,
  ProviderView,
  ModelView,
  KeyView,
  KeyTestView,
  StrategyView,
  CreateProviderPayload,
  CreateModelPayload,
  UpdateModelPayload,
  CreateKeyPayload,
  CreateKeyResponse,
  PutStrategyPayload,
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

  getKeys() {
    return alova.Get<KeyView[]>('/api/admin/keys');
  },

  getStrategy() {
    return alova.Get<StrategyView>('/api/admin/strategy');
  },

  // Write APIs (With optional If-Match optimistic concurrency headers)
  createProvider(payload: CreateProviderPayload, ifMatchVersion?: number | string) {
    return alova.Post<ProviderView>('/api/admin/providers', payload, {
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

  deleteModel(name: string, ifMatchVersion?: number | string) {
    return alova.Delete<{ ok: boolean; config_version: number }>(
      `/api/admin/models/${encodeURIComponent(name)}`,
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

  updateStrategy(payload: PutStrategyPayload, ifMatchVersion?: number | string) {
    return alova.Put<StrategyView>('/api/admin/strategy', payload, {
      headers: ifMatchHeaders(ifMatchVersion),
    });
  },
};
