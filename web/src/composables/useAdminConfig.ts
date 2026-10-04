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
  UpdateKeyPayload,
  CreateKeyResponse,
  AntigravityAuthUrlView,
  AntigravityPendingView,
  AuthorizeAntigravityPayload,
  AuthorizeAntigravityResponse,
  ProxyStatusView,
  QuotaCycleBenchmarkView,
} from '../types/admin';

export interface UseAdminConfigOptions {
  autoFetch?: boolean;
}

export const ANTIGRAVITY_QUOTA_STORAGE_KEY = 'ponyllm_antigravity_quota_results_v1';
/** 配额探测缓存 TTL：6h。超时缓存不再用于渲染水位，只保留“曾探测过”的时间戳语义。 */
export const ANTIGRAVITY_QUOTA_CACHE_TTL_MS = 6 * 3600 * 1000;

interface PersistedQuotaEnvelope {
  version: number;
  configVersion?: number;
  results: Record<string, KeyTestView>;
  probedAt: Record<string, number>;
  savedAt: number;
}

let persistedConfigVersion: number | null = null;

export function getPersistedQuotaConfigVersion(): number | null {
  return persistedConfigVersion;
}

/** 每个 key 最近一次成功/失败探测的时间戳（内存态，不持久化也可工作）。 */
const quotaProbedAt = new Map<string, number>();

export function getQuotaProbedAt(id: string): number | null {
  return quotaProbedAt.get(id) ?? null;
}

function loadPersistedQuotaResults(): Record<string, KeyTestView> {
  if (typeof window === 'undefined' || !window.localStorage) {
    return {};
  }
  try {
    const raw = window.localStorage.getItem(ANTIGRAVITY_QUOTA_STORAGE_KEY);
    if (!raw) return {};
    const data = JSON.parse(raw);
    // 兼容旧格式（裸 map）：视为已过期，仅保留结构，渲染层按 TTL 判为未知。
    if (data && typeof data === 'object' && !Array.isArray(data) && !('results' in data)) {
      const valid: Record<string, KeyTestView> = {};
      for (const [k, v] of Object.entries(data)) {
        if (v && typeof v === 'object' && ('quota' in v || 'quota_groups' in v || 'success' in v)) {
          valid[k] = v as KeyTestView;
        }
      }
      return valid;
    }
    if (data && typeof data === 'object' && !Array.isArray(data) && (data as PersistedQuotaEnvelope).results) {
      const env = data as PersistedQuotaEnvelope;
      persistedConfigVersion = env.configVersion ?? null;
      for (const [k, ts] of Object.entries(env.probedAt || {})) {
        quotaProbedAt.set(k, ts);
      }
      const valid: Record<string, KeyTestView> = {};
      for (const [k, v] of Object.entries(env.results)) {
        if (v && typeof v === 'object' && ('quota' in v || 'quota_groups' in v || 'success' in v)) {
          valid[k] = v as KeyTestView;
        }
      }
      return valid;
    }
  } catch (e) {
    console.warn('[PonyLLM] failed to load persisted quota results:', e);
  }
  return {};
}

function savePersistedQuotaResults(results: Record<string, KeyTestView>, currentConfigVersion?: number) {
  if (typeof window === 'undefined' || !window.localStorage) {
    return;
  }
  try {
    const probedAt: Record<string, number> = {};
    for (const [k, ts] of quotaProbedAt.entries()) {
      probedAt[k] = ts;
    }
    const env: PersistedQuotaEnvelope = {
      version: 1,
      configVersion: currentConfigVersion ?? persistedConfigVersion ?? undefined,
      results,
      probedAt,
      savedAt: Date.now(),
    };
    if (env.configVersion != null) {
      persistedConfigVersion = env.configVersion;
    }
    window.localStorage.setItem(ANTIGRAVITY_QUOTA_STORAGE_KEY, JSON.stringify(env));
  } catch (e) {
    console.warn('[PonyLLM] failed to save persisted quota results:', e);
  }
}

/** 探测结果是否新鲜（TTL 内）。过期结果不得用于水位渲染，只能显示占位。 */
export function isQuotaResultFresh(id: string, now: number = Date.now()): boolean {
  const ts = quotaProbedAt.get(id);
  if (ts == null) return false;
  return now - ts < ANTIGRAVITY_QUOTA_CACHE_TTL_MS;
}

/** 测试/外部调用者可手动标记某 key 的探测时间（单测 seeding 用）。 */
export function markQuotaProbed(id: string, now: number = Date.now()) {
  quotaProbedAt.set(id, now);
}

/** 单测隔离：清空内存探测时间戳（localStorage 由各测试自行清理）。 */
export function clearQuotaProbedAt() {
  quotaProbedAt.clear();
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
  const keyTestResults = ref<Record<string, KeyTestView>>(loadPersistedQuotaResults());
  /** 池级跨账号跨周期持久化累计基准（由后端快照归档提供，跨发布不归零）。 */
  const cycleBenchmark = ref<QuotaCycleBenchmarkView | null>(null);
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

      // 池级持久化累计基准：独立只读端点，失败不阻塞主流程（降级为 null）。
      void adminApi.getQuotaBenchmark().send().then((b) => {
        cycleBenchmark.value = b;
      }).catch(() => {
        cycleBenchmark.value = null;
      });

      // 增量清理已在后端移除的 key 探测缓存，避免残留数据影响视图
      const validKeyIds = new Set(kv.map((k) => k.id));
      let changed = false;
      for (const id of Object.keys(keyTestResults.value)) {
        if (!validKeyIds.has(id)) {
          delete keyTestResults.value[id];
          quotaProbedAt.delete(id);
          changed = true;
        }
      }
      if (changed) {
        savePersistedQuotaResults(keyTestResults.value, ov.config_version);
      }

      void fetchProxyStatus().catch(() => {});
    } catch (err: unknown) {
      error.value = err instanceof Error ? err.message : String(err);
      throw err;
    } finally {
      loading.value = false;
    }
  }

  /**
   * 静默刷新配置与资源列表：不在界面触发全局 loading 遮罩，仅用于用户手动刷新、
   * 写操作后对齐与冷却到期单次对齐（无自动轮询，见 GovernanceView）。
   */
  async function refreshSilent(): Promise<void> {
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
      void adminApi.getQuotaBenchmark().send().then((b) => {
        cycleBenchmark.value = b;
      }).catch(() => {
        cycleBenchmark.value = null;
      });
    } catch {
      // 静默轮询忽略瞬态网络波动
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

  async function removeModel(name: string, provider?: string): Promise<void> {
    return runWithConflictCheck(async () => {
      await adminApi.deleteModel(name, configVersion.value, provider).send();
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

  async function editKey(id: string, payload: UpdateKeyPayload): Promise<KeyView> {
    return runWithConflictCheck(async () => {
      const res = await adminApi.updateKey(id, payload, configVersion.value).send();
      const refreshedKeys = await adminApi.getKeys().send();
      keys.value = refreshedKeys;
      return res;
    });
  }

  async function removeKey(id: string): Promise<void> {
    return runWithConflictCheck(async () => {
      await adminApi.deleteKey(id, configVersion.value).send();
      if (id in keyTestResults.value) {
        delete keyTestResults.value[id];
        quotaProbedAt.delete(id);
        savePersistedQuotaResults(keyTestResults.value);
      }
      await fetchAll();
    });
  }

  async function testSingleKey(id: string): Promise<KeyTestView> {
    testingKeyIds.value.add(id);
    try {
      const res = await adminApi.testKey(id).send();
      keyTestResults.value[id] = res;
      markQuotaProbed(id);
      savePersistedQuotaResults(keyTestResults.value);
      // 拨测会由网关侧落地池动作（资格 403 → 3 天冻结等）：拨测后静默刷新 keys，
      // 让 state 与拨测结果同帧收敛，避免"红方块 + 全部就绪"自相矛盾。
      // 用 `refreshSilent`（无 loading 覆盖）而非 `fetchAll`——批量拨测 N 键
      // 不会触发 N 次全屏 loading（review 收尾建议）。
      try {
        await refreshSilent();
      } catch {
        // 刷新失败不吞拨测结果本身（网络瞬断时保留本地结果）。
      }
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
      for (let i = 0; i < list.length; i++) {
        if (i > 0) {
          await new Promise((resolve) => setTimeout(resolve, 800));
        }
        await testSingleKey(list[i].id);
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
    if (res.id && (res.quota || res.quota_groups)) {
      keyTestResults.value[res.id] = {
        success: true,
        latency_ms: 0,
        message: 'ok',
        quota: res.quota,
        quota_groups: res.quota_groups,
      };
      markQuotaProbed(res.id);
      savePersistedQuotaResults(keyTestResults.value);
    }
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
    cycleBenchmark,
    testingKeyIds,
    proxyStatus,
    batchTesting,
    fetchAll,
    refreshSilent,
    fetchProxyStatus,
    saveProvider,
    editProvider,
    removeProvider,
    saveModel,
    editModel,
    removeModel,
    addKey,
    editKey,
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
