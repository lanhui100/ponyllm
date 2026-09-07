// @vitest-environment happy-dom
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useAdminConfig } from './useAdminConfig';
import { adminApi } from '../lib/adminApi';
import { PreconditionFailedError } from '../lib/alova';
import type {
  OverviewView,
  ProviderView,
  ModelView,
  KeyView,
  StrategyView,
  CreateKeyResponse,
  KeyTestView,
} from '../types/admin';

describe('useAdminConfig composable (WEB-04 Governance & Admin CUD)', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.restoreAllMocks();
  });

  it('initializes with default empty state', () => {
    const config = useAdminConfig({ autoFetch: false });
    expect(config.overview.value).toBeNull();
    expect(config.providers.value).toEqual([]);
    expect(config.models.value).toEqual([]);
    expect(config.keys.value).toEqual([]);
    expect(config.strategy.value).toBe('economy');
    expect(config.configVersion.value).toBe(0);
    expect(config.adminWriteEnabled.value).toBe(false);
    expect(config.conflictDetected.value).toBe(false);
    expect(config.createdKeyResult.value).toBeNull();
  });

  it('fetchAll updates overview, resources, strategy and configVersion', async () => {
    const mockOverview: OverviewView = {
      version: '0.2.26',
      bind: '127.0.0.1:8080',
      auth_mode: 'token',
      providers: 1,
      keys: 2,
      keys_active: 2,
      strategy: 'balanced',
      hot_reload_ms: 1000,
      admin_write_enabled: true,
      config_version: 42,
    };

    const mockProviders: ProviderView[] = [
      {
        name: 'openai',
        base_url: 'https://api.openai.com/v1',
        default_model: 'gpt-4o',
        strategy: 'balanced',
        billing_mode: 'token',
        input_price: 2.5,
        cached_price: 1.25,
        output_price: 10.0,
        models: 1,
      },
    ];

    const mockModels: ModelView[] = [
      {
        name: 'gpt-4o',
        tier: 'Smart',
        context_window: '128k',
        thinking_default: 'Off',
        thinking_max: 'High',
      },
    ];

    const mockKeys: KeyView[] = [
      {
        id: 'key-1',
        provider: 'openai',
        masked_key: 'sk-proj-****',
        priority: 1,
        weight: 10,
        state: 'active',
      },
    ];

    const mockStrategy: StrategyView = {
      strategy: 'balanced',
      config_version: 42,
    };

    vi.spyOn(adminApi, 'getOverview').mockReturnValue({
      send: () => Promise.resolve(mockOverview),
    } as any);
    vi.spyOn(adminApi, 'getProviders').mockReturnValue({
      send: () => Promise.resolve(mockProviders),
    } as any);
    vi.spyOn(adminApi, 'getModels').mockReturnValue({
      send: () => Promise.resolve(mockModels),
    } as any);
    vi.spyOn(adminApi, 'getKeys').mockReturnValue({
      send: () => Promise.resolve(mockKeys),
    } as any);
    vi.spyOn(adminApi, 'getStrategy').mockReturnValue({
      send: () => Promise.resolve(mockStrategy),
    } as any);

    const config = useAdminConfig({ autoFetch: false });
    await config.fetchAll();

    expect(config.overview.value).toEqual(mockOverview);
    expect(config.providers.value).toEqual(mockProviders);
    expect(config.models.value).toEqual(mockModels);
    expect(config.keys.value).toEqual(mockKeys);
    expect(config.strategy.value).toBe('balanced');
    expect(config.configVersion.value).toBe(42);
    expect(config.adminWriteEnabled.value).toBe(true);
  });

  it('handles 412 PreconditionFailed by setting conflictDetected flag', async () => {
    vi.spyOn(adminApi, 'createProvider').mockReturnValue({
      send: () => Promise.reject(new PreconditionFailedError()),
    } as any);

    const config = useAdminConfig({ autoFetch: false });
    config.configVersion.value = 10;

    let threw = false;
    try {
      await config.saveProvider({ name: 'test', base_url: 'https://example.com' });
    } catch {
      threw = true;
    }

    expect(threw).toBe(true);
    expect(config.conflictDetected.value).toBe(true);

    config.clearConflict();
    expect(config.conflictDetected.value).toBe(false);
  });

  it('handles key creation and one-time plaintext key lifecycle', async () => {
    const mockCreatedKey: CreateKeyResponse = {
      id: 'key-new',
      provider: 'openai',
      api_key: 'sk-plaintext-secret-never-stored',
      priority: 1,
      weight: 10,
      state: 'active',
      config_version: 43,
    };

    vi.spyOn(adminApi, 'createKey').mockReturnValue({
      send: () => Promise.resolve(mockCreatedKey),
    } as any);
    vi.spyOn(adminApi, 'getKeys').mockReturnValue({
      send: () => Promise.resolve([]),
    } as any);

    const config = useAdminConfig({ autoFetch: false });
    config.configVersion.value = 42;

    await config.addKey({
      id: 'key-new',
      provider: 'openai',
      api_key: 'sk-plaintext-secret-never-stored',
    });

    expect(config.createdKeyResult.value).toEqual(mockCreatedKey);
    expect(config.configVersion.value).toBe(43);

    // Active destruction
    config.clearCreatedKeyResult();
    expect(config.createdKeyResult.value).toBeNull();
  });

  it('runs single key test and batch test with progress counter', async () => {
    const mockKey1Result: KeyTestView = {
      success: true,
      latency_ms: 120,
      message: 'OK',
      http_status: 200,
    };
    const mockKey2Result: KeyTestView = {
      success: false,
      latency_ms: 3000,
      message: 'dial timeout',
      http_status: 504,
      error_code: 'timeout',
    };

    vi.spyOn(adminApi, 'testKey').mockImplementation((id: string) => {
      if (id === 'k1') {
        return { send: () => Promise.resolve(mockKey1Result) } as any;
      }
      return { send: () => Promise.resolve(mockKey2Result) } as any;
    });

    const config = useAdminConfig({ autoFetch: false });
    config.keys.value = [
      { id: 'k1', provider: 'p1', masked_key: 'sk-***1', priority: 1, weight: 1, state: 'active' },
      { id: 'k2', provider: 'p1', masked_key: 'sk-***2', priority: 1, weight: 1, state: 'active' },
    ];

    // Single key test
    const res = await config.testSingleKey('k1');
    expect(res).toEqual(mockKey1Result);
    expect(config.keyTestResults.value['k1']).toEqual(mockKey1Result);

    // Batch test all keys
    await config.batchTestAllKeys();
    expect(config.batchTesting.value.running).toBe(false);
    expect(config.batchTesting.value.current).toBe(2);
    expect(config.batchTesting.value.total).toBe(2);
    expect(config.keyTestResults.value['k1'].success).toBe(true);
    expect(config.keyTestResults.value['k2'].success).toBe(false);
  });
});
