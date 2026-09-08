<script setup lang="ts">
import { computed } from 'vue';
import type { MetricsSummary } from '../types/telemetry';
import type { TelemetryPoint } from '../composables/useTelemetry';
import Icons from './ui/Icons.vue';

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
  <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4 mb-6">
    <!-- QPS 卡片 -->
    <div class="bg-white rounded-xl shadow-xs p-4.5 hover:shadow-sm transition-all duration-200">
      <div class="flex items-center justify-between text-xs text-slate-500 mb-2">
        <span class="font-medium">当前 QPS</span>
        <div class="w-6 h-6 rounded-md bg-blue-50 text-blue-600 flex items-center justify-center">
          <Icons name="activity" size="13" />
        </div>
      </div>
      <div class="text-2xl font-bold tracking-tight text-slate-900 mb-1">
        {{ qps }}
      </div>
      <div class="text-2xs text-slate-400">
        总请求数: {{ totalRequests }}
      </div>
    </div>

    <!-- Token 吞吐量卡片 -->
    <div class="bg-white rounded-xl shadow-xs p-4.5 hover:shadow-sm transition-all duration-200">
      <div class="flex items-center justify-between text-xs text-slate-500 mb-2">
        <span class="font-medium">Token 吞吐量</span>
        <div class="w-6 h-6 rounded-md bg-emerald-50 text-emerald-600 flex items-center justify-center">
          <Icons name="sparkles" size="13" />
        </div>
      </div>
      <div class="text-2xl font-bold tracking-tight text-slate-900 mb-1 flex items-baseline gap-1">
        {{ tokenThroughput }}
        <span class="text-xs font-normal text-slate-400">tok/s</span>
      </div>
      <div class="text-2xs text-slate-400">
        累计 Token: {{ totalTokens.toLocaleString() }}
      </div>
    </div>

    <!-- 延迟与生成速率 -->
    <div class="bg-white rounded-xl shadow-xs p-4.5 hover:shadow-sm transition-all duration-200">
      <div class="flex items-center justify-between text-xs text-slate-500 mb-2">
        <span class="font-medium">延迟与速率</span>
        <div class="w-6 h-6 rounded-md bg-amber-50 text-amber-600 flex items-center justify-center">
          <Icons name="zap" size="13" />
        </div>
      </div>
      <div class="text-2xl font-bold tracking-tight text-slate-900 mb-1 flex items-baseline gap-1.5 truncate">
        <span>{{ ttft }}</span>
        <span class="text-slate-300 font-light text-sm">/</span>
        <span class="text-base text-slate-600">{{ avgTps }}</span>
      </div>
      <div class="text-2xs text-slate-400">
        平均 TTFT / TPS
      </div>
    </div>

    <!-- 故障率 -->
    <div class="bg-white rounded-xl shadow-xs p-4.5 hover:shadow-sm transition-all duration-200">
      <div class="flex items-center justify-between text-xs text-slate-500 mb-2">
        <span class="font-medium">故障率</span>
        <div
          class="w-6 h-6 rounded-md flex items-center justify-center"
          :class="errorRate !== '0.0%' ? 'bg-rose-50 text-rose-600' : 'bg-slate-50 text-slate-400'"
        >
          <Icons name="cross" size="12" />
        </div>
      </div>
      <div
        class="text-2xl font-bold tracking-tight mb-1"
        :class="errorRate !== '0.0%' ? 'text-rose-600' : 'text-slate-900'"
      >
        {{ errorRate }}
      </div>
      <div class="text-2xs text-slate-400">
        失败请求: {{ metrics?.failed_requests ?? 0 }} 次
      </div>
    </div>
  </div>
</template>
