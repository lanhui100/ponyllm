<script setup lang="ts">
import { computed, ref, onMounted } from 'vue';
import { useRouter } from 'vue-router';
import { useTelemetry } from '../composables/useTelemetry';
import { useAdminConfig } from '../composables/useAdminConfig';
import NavBar from '../components/NavBar.vue';
import StatusBanner from '../components/StatusBanner.vue';
import MetricCards from '../components/MetricCards.vue';
import AntigravityPoolCard from '../components/AntigravityPoolCard.vue';
import TrendCharts from '../components/TrendCharts.vue';
import ProviderMatrix from '../components/ProviderMatrix.vue';

const router = useRouter();

const {
  keys,
  keyTestResults,
  adminWriteEnabled,
  fetchAll: fetchAdminConfig,
  testSingleKey,
} = useAdminConfig({ autoFetch: true });

const isRefreshingAntigravity = ref(false);

const antigravityKeys = computed(() => {
  return keys.value.filter((k) => k.provider.toLowerCase().includes('antigravity'));
});

async function handleRefreshAntigravityQuotas() {
  if (antigravityKeys.value.length === 0 || isRefreshingAntigravity.value) return;
  isRefreshingAntigravity.value = true;
  try {
    await Promise.allSettled(
      antigravityKeys.value.map((k) => testSingleKey(k.id))
    );
    // 探测完成后重新拉取 Key 状态：若上游已恢复额度，后端已解除冷却并推入 Active 状态
    await fetchAdminConfig();
  } finally {
    isRefreshingAntigravity.value = false;
  }
}

// 页面初始化时刷新获取 Antigravity 最新配额
onMounted(async () => {
  // 若首次 fetch 尚未完成或已有 keys，等待配置就绪后触发配额刷新
  if (antigravityKeys.value.length > 0) {
    void handleRefreshAntigravityQuotas();
  } else {
    try {
      await fetchAdminConfig();
      if (antigravityKeys.value.length > 0) {
        void handleRefreshAntigravityQuotas();
      }
    } catch {
      // 忽略初始化波动
    }
  }
});

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
  historyData,
  gatewayUptimeBars,
  setRange,
  retry,
} = useTelemetry();

const latestPoint = computed(() => {
  return history.value[history.value.length - 1];
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
      />

      <AntigravityPoolCard
        v-if="antigravityKeys.length > 0"
        :keys="antigravityKeys"
        :key-test-results="keyTestResults"
        :is-refreshing="isRefreshingAntigravity"
        :admin-write-enabled="adminWriteEnabled"
        @refresh-quotas="handleRefreshAntigravityQuotas"
        @cooldown-expired="fetchAdminConfig"
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
