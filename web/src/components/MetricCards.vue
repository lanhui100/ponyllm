<script setup lang="ts">
import { computed } from 'vue';
import type { MetricsSummary } from '../types/telemetry';
import type { TelemetryPoint } from '../composables/useTelemetry';
import Icons from './ui/Icons.vue';

const props = defineProps<{
  metrics: MetricsSummary | null;
  latestPoint?: TelemetryPoint;
}>();

const totalRequests = computed(() => {
  return props.metrics?.total_requests ?? 0;
});

const successfulRequests = computed(() => {
  return props.metrics?.successful_requests ?? 0;
});

const completionTokens = computed(() => {
  return props.metrics?.completion_tokens ?? 0;
});

const promptTokens = computed(() => {
  return props.metrics?.prompt_tokens ?? 0;
});

const totalTokens = computed(() => {
  return props.metrics?.total_tokens ?? 0;
});

const ttft = computed(() => {
  const v = props.metrics?.stream?.avg_ttft_ms;
  return v !== undefined && v !== null && v > 0 ? `${Math.round(v)} ms` : '--';
});

const avgTps = computed(() => {
  const v = props.metrics?.stream?.avg_tps;
  return v !== undefined && v !== null && v > 0 ? `${Math.round(v)} tok/s` : '--';
});

const errorRate = computed(() => {
  if (!props.metrics || props.metrics.total_requests === 0) return '0.0%';
  const rate = (props.metrics.failed_requests / props.metrics.total_requests) * 100;
  return `${rate.toFixed(1)}%`;
});
</script>

<template>
  <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-5 gap-4 mb-6">
    <!-- 1. 调用次数 -->
    <div class="swiss-card p-5 bg-orange-50/40 transition-all duration-200">
      <div class="flex items-center justify-between text-[13px] text-slate-500 mb-2.5">
        <span class="font-semibold text-slate-700">调用次数</span>
        <div class="w-7.5 h-7.5 rounded-lg bg-orange-100 text-orange-600 flex items-center justify-center">
          <Icons name="activity" size="16" />
        </div>
      </div>
      <div class="text-3xl font-bold tracking-tight text-slate-900 mb-1 font-mono tabular-nums">
        {{ totalRequests.toLocaleString() }}
      </div>
      <div class="text-[13px] text-slate-500 font-medium">
        成功调用: <span class="font-mono text-slate-700">{{ successfulRequests.toLocaleString() }} 次</span>
      </div>
    </div>

    <!-- 2. Token总计 (token生成的数量) -->
    <div class="swiss-card p-5 bg-sky-50/50 transition-all duration-200">
      <div class="flex items-center justify-between text-[13px] text-slate-500 mb-2.5">
        <span class="font-semibold text-slate-700">Token 总计</span>
        <div class="w-7.5 h-7.5 rounded-lg bg-sky-100 text-sky-700 flex items-center justify-center">
          <Icons name="sparkles" size="16" />
        </div>
      </div>
      <div class="text-3xl font-bold tracking-tight text-slate-900 mb-1 flex items-baseline gap-1.5 font-mono tabular-nums">
        {{ completionTokens.toLocaleString() }}
        <span class="text-sm font-normal text-slate-500 font-sans">tok</span>
      </div>
      <div class="text-[13px] text-slate-500 font-medium truncate" :title="`输入: ${promptTokens.toLocaleString()} · 总消耗: ${totalTokens.toLocaleString()}`">
        输入: <span class="font-mono text-slate-700">{{ promptTokens.toLocaleString() }}</span> · 总消耗: <span class="font-mono text-slate-700">{{ totalTokens.toLocaleString() }}</span>
      </div>
    </div>

    <!-- 3. 延迟 -->
    <div class="swiss-card p-5 bg-amber-50/50 transition-all duration-200">
      <div class="flex items-center justify-between text-[13px] text-slate-500 mb-2.5">
        <span class="font-semibold text-slate-700">延迟</span>
        <div class="w-7.5 h-7.5 rounded-lg bg-amber-100 text-amber-600 flex items-center justify-center">
          <Icons name="zap" size="16" />
        </div>
      </div>
      <div class="text-3xl font-bold tracking-tight text-slate-900 mb-1 font-mono tabular-nums">
        {{ ttft }}
      </div>
      <div class="text-[13px] text-slate-500 font-medium">
        平均首字延迟 (TTFT)
      </div>
    </div>

    <!-- 4. 速率 -->
    <div class="swiss-card p-5 bg-teal-50/50 transition-all duration-200">
      <div class="flex items-center justify-between text-[13px] text-slate-500 mb-2.5">
        <span class="font-semibold text-slate-700">速率</span>
        <div class="w-7.5 h-7.5 rounded-lg bg-teal-100 text-teal-700 flex items-center justify-center">
          <Icons name="sparkles" size="16" />
        </div>
      </div>
      <div class="text-3xl font-bold tracking-tight text-slate-900 mb-1 font-mono tabular-nums">
        {{ avgTps }}
      </div>
      <div class="text-[13px] text-slate-500 font-medium">
        平均生成速率 (TPS)
      </div>
    </div>

    <!-- 5. 故障率 -->
    <div class="swiss-card p-5 bg-slate-100/70 transition-all duration-200">
      <div class="flex items-center justify-between text-[13px] text-slate-500 mb-2.5">
        <span class="font-semibold text-slate-700">故障率</span>
        <div
          class="w-7.5 h-7.5 rounded-lg flex items-center justify-center"
          :class="errorRate !== '0.0%' ? 'bg-rose-100 text-rose-600' : 'bg-slate-200/70 text-slate-500'"
        >
          <Icons name="cross" size="15" />
        </div>
      </div>
      <div
        class="text-3xl font-bold tracking-tight mb-1 font-mono tabular-nums"
        :class="errorRate !== '0.0%' ? 'text-rose-600' : 'text-slate-900'"
      >
        {{ errorRate }}
      </div>
      <div class="text-[13px] text-slate-500 font-medium">
        失败请求: <span class="font-mono text-slate-700">{{ metrics?.failed_requests ?? 0 }} 次</span>
      </div>
    </div>
  </div>
</template>
