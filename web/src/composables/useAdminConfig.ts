import { ref, computed, onMounted, getCurrentInstance } from 'vue';
import { adminApi } from '../lib/adminApi';
import { isPreconditionFailed } from '../lib/alova';
import type {
  OverviewView,
  ServiceStatusView,
  ProviderView,
  ModelView,
  KeyView,
  KeyTestView,
  CreateProviderPayload,
  UpdateProviderPayload,
  CreateModelPayload,
  UpdateModelPayload,
  CreateKeyPayload,
  CreateKeyResponse,
  AntigravityAuthUrlView,
  AntigravityPendingView,
  AuthorizeAntigravityPayload,
  AuthorizeAntigravityResponse,
  ProxyStatusView,
} from '../types/admin';

export interface UseAdminConfigOptions {
  autoFetch?: boolean;
}

export function useAdminConfig(options: UseAdminConfigOptions = {}) {
  const { autoFetch = true } = options;

  const overview = ref<OverviewView | null>(null);
  const serviceStatus = ref<ServiceStatusView | null>(null);
  const providers = ref<ProviderView[]>([]);
  const models = ref<ModelView[]>([]);
  const keys = ref<KeyView[]>([]);
  const strategy = ref<string>('economy');
  const configVersion = ref<number>(0);
  const loading = ref<boolean>(false);
  const error = ref<string | null>(null);

  const conflictDetected = ref<boolean>(false);
  const createdKeyResult = ref<CreateKeyResponse | null>(null);
  const keyTestResults = ref<Record<string, KeyTestView>>({});
  const testingKeyIds = ref<Set<string>>(new Set());
  const proxyStatus = ref<ProxyStatusView | null>(null);
  const batchTesting = ref<{ running: boolean; current: number; total: number }>({
    running: false,
    current: 0,
    total: 0,
  });

  const adminWriteEnabled = computed(() => {
    if (overview.value !== null) {
      return overview.value.admin_write_enabled;
    }
    if (serviceStatus.value !== null) {
      return serviceStatus.value.admin_write_enabled;
    }
    return false;
  });

  async function fetchProxyStatus(): Promise<ProxyStatusView | null> {
    try {
      const ps = await adminApi.getProxyStatus().send();
      proxyStatus.value = ps;
      return ps;
    } catch {
      return null;
    }
  }

  async function fetchAll(): Promise<void> {
    loading.value = true;
    error.value = null;
    try {
      const [ov, pv, mv, kv, st] = await Promise.all([
        adminApi.getOverview().send(),
        adminApi.getProviders().send(),
        adminApi.getModels().send(),
        adminApi.getKeys().send(),
        adminApi.getStrategy().send(),
      ]);

      overview.value = ov;
      providers.value = pv;
      models.value = mv;
      keys.value = kv;
      strategy.value = st.strategy;
      configVersion.value = ov.config_version;

      void fetchProxyStatus().catch(() => {});
    } catch (err: unknown) {
      error.value = err instanceof Error ? err.message : String(err);
      throw err;
    } finally {
      loading.value = false;
    }
  }

  async function runWithConflictCheck<T>(fn: () => Promise<T>): Promise<T> {
    try {
      return await fn();
    } catch (err: unknown) {
      if (!isPreconditionFailed(err)) throw err;
      // 版本过期多半是系统后台任务（如密钥自动续期写盘）碰了配置，并非真的
      // 有人抢改：先刷新到最新自动重试一次，仍冲突才认为是真正的并发修改。
      try {
        await fetchAll();
      } catch {
        // 刷新失败也不阻塞重试，交由第二次提交的结果定夺
      }
      try {
        return await fn();
      } catch (retryErr: unknown) {
        if (isPreconditionFailed(retryErr)) {
          conflictDetected.value = true;
        }
        throw retryErr;
      }
    }
  }

  async function saveProvider(payload: CreateProviderPayload): Promise<ProviderView> {
    return runWithConflictCheck(async () => {
      const res = await adminApi.createProvider(payload, configVersion.value).send();
      await fetchAll();
      return res;
    });
  }

  async function editProvider(name: string, payload: UpdateProviderPayload): Promise<ProviderView> {
    return runWithConflictCheck(async () => {
      const res = await adminApi.updateProvider(name, payload, configVersion.value).send();
      await fetchAll();
      return res;
    });
  }

  async function removeProvider(name: string): Promise<void> {
    return runWithConflictCheck(async () => {
      await adminApi.deleteProvider(name, configVersion.value).send();
      await fetchAll();
    });
  }

  async function saveModel(payload: CreateModelPayload): Promise<ModelView> {
    return runWithConflictCheck(async () => {
      const res = await adminApi.createModel(payload, configVersion.value).send();
      await fetchAll();
      return res;
    });
  }

  async function editModel(name: string, payload: UpdateModelPayload): Promise<ModelView> {
    return runWithConflictCheck(async () => {
      const res = await adminApi.updateModel(name, payload, configVersion.value).send();
      await fetchAll();
      return res;
    });
  }

  async function removeModel(name: string): Promise<void> {
    return runWithConflictCheck(async () => {
      await adminApi.deleteModel(name, configVersion.value).send();
      await fetchAll();
    });
  }

  async function addKey(payload: CreateKeyPayload): Promise<CreateKeyResponse> {
    return runWithConflictCheck(async () => {
      const res = await adminApi.createKey(payload, configVersion.value).send();
      createdKeyResult.value = res;
      configVersion.value = res.config_version;
      const refreshedKeys = await adminApi.getKeys().send();
      keys.value = refreshedKeys;
      return res;
    });
  }

  async function removeKey(id: string): Promise<void> {
    return runWithConflictCheck(async () => {
      await adminApi.deleteKey(id, configVersion.value).send();
      await fetchAll();
    });
  }

  async function testSingleKey(id: string): Promise<KeyTestView> {
    testingKeyIds.value.add(id);
    try {
      const res = await adminApi.testKey(id).send();
      keyTestResults.value[id] = res;
      return res;
    } finally {
      testingKeyIds.value.delete(id);
    }
  }

  async function batchTestAllKeys(): Promise<void> {
    const list = keys.value;
    if (list.length === 0) return;

    batchTesting.value = {
      running: true,
      current: 0,
      total: list.length,
    };

    try {
      for (const k of list) {
        await testSingleKey(k.id);
        batchTesting.value.current += 1;
      }
    } finally {
      batchTesting.value.running = false;
    }
  }

  async function saveStrategy(newStrategy: string): Promise<void> {
    return runWithConflictCheck(async () => {
      const res = await adminApi.updateStrategy({ strategy: newStrategy }, configVersion.value).send();
      strategy.value = res.strategy;
      configVersion.value = res.config_version;
    });
  }

  function clearConflict(): void {
    conflictDetected.value = false;
  }

  function clearCreatedKeyResult(): void {
    createdKeyResult.value = null;
  }

  async function getAntigravityAuthUrl(redirectUri?: string, state?: string): Promise<AntigravityAuthUrlView> {
    return adminApi.getAntigravityAuthUrl(redirectUri, state).send();
  }

  async function getAntigravityPending(state: string): Promise<AntigravityPendingView> {
    return adminApi.getAntigravityPending(state).send();
  }

  async function authorizeAntigravity(payload: AuthorizeAntigravityPayload): Promise<AuthorizeAntigravityResponse> {
    const res = await adminApi.authorizeAntigravity(payload).send();
    await fetchAll();
    return res;
  }

  if (autoFetch && getCurrentInstance()) {
    onMounted(() => {
      void fetchAll().catch(() => {});
    });
  }

  return {
    overview,
    serviceStatus,
    providers,
    models,
    keys,
    strategy,
    configVersion,
    loading,
    error,
    adminWriteEnabled,
    conflictDetected,
    createdKeyResult,
    keyTestResults,
    testingKeyIds,
    proxyStatus,
    batchTesting,
    fetchAll,
    fetchProxyStatus,
    saveProvider,
    editProvider,
    removeProvider,
    saveModel,
    editModel,
    removeModel,
    addKey,
    removeKey,
    testSingleKey,
    batchTestAllKeys,
    saveStrategy,
    clearConflict,
    clearCreatedKeyResult,
    getAntigravityAuthUrl,
    getAntigravityPending,
    authorizeAntigravity,
  };
}
