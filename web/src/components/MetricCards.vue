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
    <div class="borderless-card p-5 hover:shadow-md transition-all duration-200">
      <div class="flex items-center justify-between text-xs text-slate-500 mb-2.5">
        <span class="font-semibold text-slate-600">当前 QPS</span>
        <div class="w-7 h-7 rounded-lg bg-orange-50 text-orange-600 flex items-center justify-center">
          <Icons name="activity" size="15" />
        </div>
      </div>
      <div class="text-3xl font-bold tracking-tight text-slate-900 mb-1 font-mono">
        {{ qps }}
      </div>
      <div class="text-xs text-slate-400 font-medium">
        总请求数: {{ totalRequests }}
      </div>
    </div>

    <!-- Token 吞吐量卡片 -->
    <div class="borderless-card p-5 hover:shadow-md transition-all duration-200">
      <div class="flex items-center justify-between text-xs text-slate-500 mb-2.5">
        <span class="font-semibold text-slate-600">Token 吞吐量</span>
        <div class="w-7 h-7 rounded-lg bg-sky-50 text-sky-700 flex items-center justify-center">
          <Icons name="sparkles" size="15" />
        </div>
      </div>
      <div class="text-3xl font-bold tracking-tight text-slate-900 mb-1 flex items-baseline gap-1.5 font-mono">
        {{ tokenThroughput }}
        <span class="text-xs font-normal text-slate-400 font-sans">tok/s</span>
      </div>
      <div class="text-xs text-slate-400 font-medium">
        累计 Token: {{ totalTokens.toLocaleString() }}
      </div>
    </div>

    <!-- 延迟与生成速率 -->
    <div class="borderless-card p-5 hover:shadow-md transition-all duration-200">
      <div class="flex items-center justify-between text-xs text-slate-500 mb-2.5">
        <span class="font-semibold text-slate-600">延迟与速率</span>
        <div class="w-7 h-7 rounded-lg bg-amber-50 text-amber-600 flex items-center justify-center">
          <Icons name="zap" size="15" />
        </div>
      </div>
      <div class="text-3xl font-bold tracking-tight text-slate-900 mb-1 flex flex-wrap items-baseline gap-1.5 font-mono">
        <span>{{ ttft }}</span>
        <span class="text-slate-300 font-light text-base">/</span>
        <span class="text-lg text-slate-600 font-medium whitespace-nowrap">{{ avgTps }}</span>
      </div>
      <div class="text-xs text-slate-400 font-medium">
        平均 TTFT / TPS
      </div>
    </div>

    <!-- 故障率 -->
    <div class="borderless-card p-5 hover:shadow-md transition-all duration-200">
      <div class="flex items-center justify-between text-xs text-slate-500 mb-2.5">
        <span class="font-semibold text-slate-600">故障率</span>
        <div
          class="w-7 h-7 rounded-lg flex items-center justify-center"
          :class="errorRate !== '0.0%' ? 'bg-rose-50 text-rose-600' : 'bg-slate-100 text-slate-400'"
        >
          <Icons name="cross" size="14" />
        </div>
      </div>
      <div
        class="text-3xl font-bold tracking-tight mb-1 font-mono"
        :class="errorRate !== '0.0%' ? 'text-rose-600' : 'text-slate-900'"
      >
        {{ errorRate }}
      </div>
      <div class="text-xs text-slate-400 font-medium">
        失败请求: {{ metrics?.failed_requests ?? 0 }} 次
      </div>
    </div>
  </div>
</template>
