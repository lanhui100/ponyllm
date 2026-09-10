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

function formatTokensInt(n: number): string {
  if (n >= 1_000_000) return `${Math.round(n / 1_000_000)}M`;
  if (n >= 1_000) return `${Math.round(n / 1_000)}K`;
  return Math.round(n).toString();
}

const completionTokens = computed(() => {
  return props.metrics?.completion_tokens ?? 0;
});

const promptTokens = computed(() => {
  return props.metrics?.prompt_tokens ?? 0;
});

const cachedTokens = computed(() => {
  return props.metrics?.cached_tokens ?? 0;
});

const totalTokens = computed(() => {
  return props.metrics?.total_tokens ?? 0;
});

const cacheHitRate = computed(() => {
  const prompt = promptTokens.value;
  const cached = cachedTokens.value;
  if (prompt <= 0) return '0%';
  const rate = Math.min(100, Math.round((cached / prompt) * 100));
  return `${rate}%`;
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
    <div class="swiss-card p-5 bg-white/45 backdrop-blur-xs transition-all duration-200 border border-white/40">
      <div class="flex items-center justify-between text-[13px] text-slate-500 mb-2.5">
        <span class="font-semibold text-slate-700 inline-flex items-center gap-1.5">
          <Icons name="activity" size="16" class="text-orange-600" />
          调用次数
        </span>
      </div>
      <div class="text-3xl font-bold tracking-tight text-slate-900 mb-1 font-mono tabular-nums">
        {{ totalRequests.toLocaleString() }}
      </div>
      <div class="text-[13px] text-slate-500 font-medium">
        成功调用: <span class="font-mono text-slate-700">{{ successfulRequests.toLocaleString() }} 次</span>
      </div>
    </div>

    <!-- 2. Token量 (主数字输出总计，下方输入/输出/缓存三维度) -->
    <div class="swiss-card p-5 bg-white/45 backdrop-blur-xs transition-all duration-200 border border-white/40">
      <div class="flex items-center justify-between text-[13px] text-slate-500 mb-1.5">
        <span class="font-semibold text-slate-700 inline-flex items-center gap-1.5">
          <Icons name="sparkles" size="16" class="text-sky-700" />
          Token量
        </span>
      </div>
      <!-- 主数字：输出总计 -->
      <div class="text-3xl font-bold tracking-tight text-slate-900 mb-1.5 flex items-baseline gap-1.5 font-mono tabular-nums">
        {{ completionTokens.toLocaleString() }}
        <span class="text-sm font-normal text-slate-500 font-sans">tok</span>
      </div>
      <!-- 下方：输入、输出、缓存三个维度 (以K/M整数单位呈现，缓存附带百分比) -->
      <div class="text-[12px] text-slate-600 font-medium flex flex-wrap items-center gap-x-2 gap-y-0.5" :title="`输入: ${promptTokens.toLocaleString()} · 输出: ${completionTokens.toLocaleString()} · 缓存: ${cachedTokens.toLocaleString()} (${cacheHitRate})`">
        <span>输入: <span class="font-mono text-slate-800 font-semibold">{{ formatTokensInt(promptTokens) }}</span></span>
        <span class="text-slate-300">·</span>
        <span>输出: <span class="font-mono text-slate-800 font-semibold">{{ formatTokensInt(completionTokens) }}</span></span>
        <span class="text-slate-300">·</span>
        <span>缓存: <span class="font-mono text-emerald-700 font-semibold">{{ formatTokensInt(cachedTokens) }}</span> <span class="text-3xs font-mono text-emerald-600 font-bold ml-0.5">({{ cacheHitRate }})</span></span>
      </div>
    </div>

    <!-- 3. 延迟 -->
    <div class="swiss-card p-5 bg-white/45 backdrop-blur-xs transition-all duration-200 border border-white/40">
      <div class="flex items-center justify-between text-[13px] text-slate-500 mb-2.5">
        <span class="font-semibold text-slate-700 inline-flex items-center gap-1.5">
          <Icons name="zap" size="16" class="text-amber-600" />
          延迟
        </span>
      </div>
      <div class="text-3xl font-bold tracking-tight text-slate-900 mb-1 font-mono tabular-nums">
        {{ ttft }}
      </div>
      <div class="text-[13px] text-slate-500 font-medium">
        平均首字延迟 (TTFT)
      </div>
    </div>

    <!-- 4. 速率 -->
    <div class="swiss-card p-5 bg-white/45 backdrop-blur-xs transition-all duration-200 border border-white/40">
      <div class="flex items-center justify-between text-[13px] text-slate-500 mb-2.5">
        <span class="font-semibold text-slate-700 inline-flex items-center gap-1.5">
          <Icons name="sparkles" size="16" class="text-teal-700" />
          速率
        </span>
      </div>
      <div class="text-3xl font-bold tracking-tight text-slate-900 mb-1 font-mono tabular-nums">
        {{ avgTps }}
      </div>
      <div class="text-[13px] text-slate-500 font-medium">
        平均生成速率 (TPS)
      </div>
    </div>

    <!-- 5. 故障率 -->
    <div class="swiss-card p-5 bg-white/45 backdrop-blur-xs transition-all duration-200 border border-white/40">
      <div class="flex items-center justify-between text-[13px] text-slate-500 mb-2.5">
        <span class="font-semibold text-slate-700 inline-flex items-center gap-1.5">
          <Icons
            name="warning"
            size="15"
            :class="errorRate !== '0.0%' ? 'text-rose-600' : 'text-slate-500'"
          />
          故障率
        </span>
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
