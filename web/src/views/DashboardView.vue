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
  retry,
} = useTelemetry();

const latestPoint = computed(() => {
  return history.value[history.value.length - 1];
});
</script>

<template>
  <div class="dashboard-page">
    <NavBar />

    <main class="page-content" :class="{ 'is-down': isDown }">
      <div class="header-row">
        <div>
          <h1 class="page-title">系统可观测大盘</h1>
          <p class="page-desc">实时遥测流与网关核心性能指标</p>
        </div>
      </div>

      <StatusBanner
        :health="health"
        :transport="transport"
        :is-down="isDown"
        @retry="retry"
      />

      <MetricCards
        :metrics="metrics"
        :latest-point="latestPoint"
      />

      <TrendCharts
        :history="history"
      />

      <ProviderMatrix
        :providers="stream?.providers"
      />
    </main>
  </div>
</template>

<style scoped>
.dashboard-page {
  min-height: 100vh;
  background: #f8fafc;
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
  margin-bottom: 20px;
}

.page-title {
  font-size: 20px;
  font-weight: 700;
  color: #0f172a;
  margin: 0 0 4px 0;
}

.page-desc {
  font-size: 13px;
  color: #64748b;
  margin: 0;
}
</style>
