<script setup lang="ts">
import { computed } from 'vue';
import type { MetricsSummary } from '../types/telemetry';
import type { TelemetryPoint } from '../composables/useTelemetry';

const props = defineProps<{
  metrics: MetricsSummary | null;
  latestPoint?: TelemetryPoint;
}>();

const qps = computed(() => {
  return props.latestPoint?.qps ?? 0;
});

const tokenThroughput = computed(() => {
  return props.latestPoint?.tokenThroughput ?? 0;
});

const totalTokens = computed(() => {
  return props.metrics?.total_tokens ?? 0;
});

const ttft = computed(() => {
  const v = props.metrics?.stream?.avg_ttft_ms;
  return v !== undefined && v !== null ? `${v.toFixed(1)} ms` : '--';
});

const avgTps = computed(() => {
  const v = props.metrics?.stream?.avg_tps;
  return v !== undefined && v !== null ? `${v.toFixed(1)} tok/s` : '--';
});

const errorRate = computed(() => {
  if (!props.metrics || props.metrics.total_requests === 0) return '0.0%';
  const rate = (props.metrics.failed_requests / props.metrics.total_requests) * 100;
  return `${rate.toFixed(1)}%`;
});

const totalRequests = computed(() => {
  return props.metrics?.total_requests ?? 0;
});
</script>

<template>
  <div class="kpi-grid">
    <div class="kpi-card">
      <div class="card-title">当前 QPS</div>
      <div class="card-value">{{ qps }}</div>
      <div class="card-sub">总请求数: {{ totalRequests }}</div>
    </div>

    <div class="kpi-card">
      <div class="card-title">Token 吞吐量</div>
      <div class="card-value">{{ tokenThroughput }} <span class="unit">tok/s</span></div>
      <div class="card-sub">累计 Token: {{ totalTokens.toLocaleString() }}</div>
    </div>

    <div class="kpi-card">
      <div class="card-title">延迟与生成速率</div>
      <div class="card-value">{{ ttft }} <span class="divider">/</span> {{ avgTps }}</div>
      <div class="card-sub">平均 TTFT / TPS</div>
    </div>

    <div class="kpi-card">
      <div class="card-title">故障率</div>
      <div class="card-value" :class="{ 'has-errors': errorRate !== '0.0%' }">{{ errorRate }}</div>
      <div class="card-sub">失败请求: {{ metrics?.failed_requests ?? 0 }} 次</div>
    </div>
  </div>
</template>

<style scoped>
.kpi-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: 16px;
  margin-bottom: 24px;
}

.kpi-card {
  background: #ffffff;
  border: 1px solid #e2e8f0;
  border-radius: 8px;
  padding: 16px 20px;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.04);
}

.card-title {
  font-size: 13px;
  font-weight: 500;
  color: #64748b;
  margin-bottom: 8px;
}

.card-value {
  font-size: 24px;
  font-weight: 600;
  color: #0f172a;
  display: flex;
  align-items: baseline;
  gap: 4px;
}

.card-value.has-errors {
  color: #ef4444;
}

.unit {
  font-size: 14px;
  font-weight: normal;
  color: #64748b;
}

.divider {
  font-size: 16px;
  color: #cbd5e1;
  margin: 0 2px;
}

.card-sub {
  font-size: 12px;
  color: #94a3b8;
  margin-top: 6px;
}
</style>
