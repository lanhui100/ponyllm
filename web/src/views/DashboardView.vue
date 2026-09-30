<script setup lang="ts">
import { computed, ref, onMounted } from 'vue';
import { useRouter } from 'vue-router';
import { useTelemetry } from '../composables/useTelemetry';
import { useAdminConfig, isQuotaResultFresh } from '../composables/useAdminConfig';
import { isAntigravityScope } from '../utils/antigravityQuota';
import NavBar from '../components/NavBar.vue';
import StatusBanner from '../components/StatusBanner.vue';
import MetricCards from '../components/MetricCards.vue';
import DashboardSkeleton from '../components/DashboardSkeleton.vue';
import AntigravityPoolCard from '../components/AntigravityPoolCard.vue';
import TrendCharts from '../components/TrendCharts.vue';
import ProviderMatrix from '../components/ProviderMatrix.vue';

const router = useRouter();

const {
  keys,
  providers,
  keyTestResults,
  adminWriteEnabled,
  fetchAll: fetchAdminConfig,
  testSingleKey,
} = useAdminConfig({ autoFetch: true });

const isRefreshingAntigravity = ref(false);

const antigravityKeys = computed(() => {
  const providerMap = new Map<string, string | null>();
  for (const p of providers.value) {
    providerMap.set(p.name, p.default_protocol ?? null);
  }
  return keys.value.filter((k) =>
    isAntigravityScope(k.provider, providerMap.get(k.provider))
  );
});

async function handleRefreshAntigravityQuotas() {
  if (antigravityKeys.value.length === 0 || isRefreshingAntigravity.value) return;
  isRefreshingAntigravity.value = true;
  try {
    // 并发度为 2 的受控错峰探测队列，兼顾探测性能与跨节点锁平滑度
    const queue = [...antigravityKeys.value];
    const concurrency = Math.min(2, queue.length);
    const workers = Array.from({ length: concurrency }, async (_, workerIdx) => {
      if (workerIdx > 0) {
        await new Promise((resolve) => setTimeout(resolve, workerIdx * 150));
      }
      while (queue.length > 0) {
        const k = queue.shift();
        if (!k) break;
        await testSingleKey(k.id);
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
    });
    await Promise.all(workers);

    // 检查是否有由于跨节点锁冲突处于 lock_busy 状态的 Key，自动延迟 1.5s 补测收敛一次
    const busyKeys = antigravityKeys.value.filter((k) => {
      const res = keyTestResults.value[k.id];
      const errCode = (res?.error_code || '').toLowerCase();
      const errMsg = (res?.message || '').toLowerCase();
      return errCode.includes('lock_busy') || errMsg.includes('serialization lock') || errMsg.includes('held by another replica');
    });

    if (busyKeys.length > 0) {
      await new Promise((resolve) => setTimeout(resolve, 1500));
      for (const k of busyKeys) {
        await testSingleKey(k.id);
        await new Promise((resolve) => setTimeout(resolve, 150));
      }
    }

    // 探测完成后重新拉取 Key 状态：若上游已恢复额度，后端已解除冷却并推入 Active 状态
    await fetchAdminConfig();
  } finally {
    isRefreshingAntigravity.value = false;
  }
}

// 页面初始化：仅对"无新鲜缓存"的 key 补测，已有新鲜缓存不再自动打上游（防风控）。
// 全量刷新只走用户手动"刷新配额"按钮。
onMounted(async () => {
  // 若首次 fetch 尚未完成或已有 keys，等待配置就绪后触发配额刷新
  if (antigravityKeys.value.length > 0) {
    void handleRefreshMissingQuotas();
  } else {
    try {
      await fetchAdminConfig();
      if (antigravityKeys.value.length > 0) {
        void handleRefreshMissingQuotas();
      }
    } catch {
      // 忽略初始化波动
    }
  }
});

// 冷却到期刷新：带 30s 防抖 + 可见性守卫，避免多个 key 到期造成高频重复拉取
let lastCooldownSyncAt = 0;
function handleCooldownExpired() {
  if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return;
  const now = Date.now();
  if (now - lastCooldownSyncAt < 30000) return;
  lastCooldownSyncAt = now;
  void fetchAdminConfig();
}

async function handleRefreshMissingQuotas() {
  if (!adminWriteEnabled.value) return;
  if (antigravityKeys.value.length === 0 || isRefreshingAntigravity.value) return;
  const missing = antigravityKeys.value.filter((k) => !isQuotaResultFresh(k.id));
  if (missing.length === 0) return;
  isRefreshingAntigravity.value = true;
  try {
    // 串行补测 + 800ms 间隔，避免并发打 Google 上游配额接口
    for (const k of missing) {
      await testSingleKey(k.id);
      await new Promise((resolve) => setTimeout(resolve, 800));
    }
    await fetchAdminConfig();
  } finally {
    isRefreshingAntigravity.value = false;
  }
}

function handleNavigateGovernance() {
  router.push('/governance');
}

const {
  health,
  metrics,
  stream,
  history,
  transport,
  isDown,
  selectedRange,
  isHistoryLoading,
  historyData,
  gatewayUptimeBars,
  setRange,
  retry,
} = useTelemetry();

const latestPoint = computed(() => {
  return history.value[history.value.length - 1];
});

/** 首屏骨架：三路遥测均未到达时展示，一旦有过数据则永不回骨架（避免轮询闪烁）。 */
const showSkeleton = computed(() => {
  return health.value === 'unknown' && metrics.value === null && stream.value === null;
});

const speed24h = computed<number | undefined>(() => {
  const points = historyData.value?.points;
  if (points && points.length > 0) {
    const totalTps = points.reduce((acc: number, b) => acc + (b.token_throughput ?? 0), 0);
    return Math.round(totalTps / points.length);
  }
  if (metrics.value?.stream?.avg_tps !== undefined && metrics.value.stream.avg_tps !== null) {
    return Math.round(metrics.value.stream.avg_tps);
  }
  return 0;
});
</script>

<template>
  <div class="dashboard-page">
    <NavBar />

    <main class="page-content" :class="{ 'is-down': isDown }">
      <div class="header-row">
        <div>
          <h1 class="page-title">系统仪表盘</h1>
          <p class="page-desc">实时遥测流、上游节点连通性微柱与多周期指标分析</p>
        </div>
      </div>

      <DashboardSkeleton v-if="showSkeleton" />

      <template v-else>
      <StatusBanner
        :health="health"
        :transport="transport"
        :is-down="isDown"
        :uptime-bars="gatewayUptimeBars"
        :speed-24h="speed24h"
        @retry="retry"
      />

      <MetricCards
        :metrics="metrics"
        :latest-point="latestPoint"
        :range="selectedRange"
        :history-data="historyData"
        :loading="isHistoryLoading"
        @update:range="setRange"
      />

      <AntigravityPoolCard
        v-if="antigravityKeys.length > 0"
        :keys="antigravityKeys"
        :key-test-results="keyTestResults"
        :is-refreshing="isRefreshingAntigravity"
        :admin-write-enabled="adminWriteEnabled"
        @refresh-quotas="handleRefreshAntigravityQuotas"
        @cooldown-expired="handleCooldownExpired"
        @navigate-governance="handleNavigateGovernance"
      />

      <TrendCharts
        :history="history"
        :history-data="historyData"
        :range="selectedRange"
        @update:range="setRange"
      />

      <ProviderMatrix
        :providers="stream?.providers"
        :range="selectedRange"
        :provider-tokens="historyData?.provider_tokens"
        :provider-prompt-tokens="historyData?.provider_prompt_tokens"
        :provider-completion-tokens="historyData?.provider_completion_tokens"
        :provider-cached-tokens="historyData?.provider_cached_tokens"
        @update:range="setRange"
      />
      </template>
    </main>
  </div>
</template>

<style scoped>
.dashboard-page {
  min-height: 100vh;
  background: transparent;
}

.page-content {
  max-width: 1280px;
  margin: 0 auto;
  padding: 24px;
  transition: filter 0.3s ease;
}

.page-content.is-down {
  filter: grayscale(0.85);
  pointer-events: auto;
}

.header-row {
  margin-bottom: 24px;
}

.page-title {
  font-size: 24px;
  font-weight: 700;
  color: #0f172a;
  margin: 0 0 6px 0;
  letter-spacing: -0.02em;
}

.page-desc {
  font-size: 14px;
  color: #64748b;
  margin: 0;
}
</style>
