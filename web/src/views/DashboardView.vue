<script setup lang="ts">
import { computed } from 'vue';
import { useTelemetry } from '../composables/useTelemetry';
import NavBar from '../components/NavBar.vue';
import StatusBanner from '../components/StatusBanner.vue';
import MetricCards from '../components/MetricCards.vue';
import TrendCharts from '../components/TrendCharts.vue';
import ProviderMatrix from '../components/ProviderMatrix.vue';

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
          <h1 class="page-title">系统可观测大盘</h1>
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
        @update:range="setRange"
      />
    </main>
  </div>
</template>

<style scoped>
.dashboard-page {
  min-height: 100vh;
  background: transparent;
  font-family: system-ui, -apple-system, sans-serif;
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
