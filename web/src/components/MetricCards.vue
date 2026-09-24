<script setup lang="ts">
import { computed } from 'vue';
import type { MetricsSummary, TimeseriesHistoryResponse } from '../types/telemetry';
import type { TelemetryPoint } from '../composables/useTelemetry';
import Icons from './ui/Icons.vue';

const props = defineProps<{
  metrics: MetricsSummary | null;
  latestPoint?: TelemetryPoint;
  range?: '24h' | '7d' | '30d' | 'all';
  historyData?: TimeseriesHistoryResponse | null;
  loading?: boolean;
}>();

const emit = defineEmits<{
  (e: 'update:range', range: '24h' | '7d' | '30d' | 'all'): void;
}>();

const rangeOptions: Array<{ key: '24h' | '7d' | '30d' | 'all'; label: string }> = [
  { key: '24h', label: '今日 (24h)' },
  { key: '7d', label: '7天' },
  { key: '30d', label: '当月 (30d)' },
  { key: 'all', label: '全部' },
];

const isAllTime = computed(() => (props.range ?? '24h') === 'all');

const totalRequests = computed(() => {
  if (isAllTime.value || !props.historyData) {
    return props.metrics?.total_requests ?? 0;
  }
  return props.historyData.total_requests ?? 0;
});

const failedRequests = computed(() => {
  if (isAllTime.value || !props.historyData) {
    return props.metrics?.failed_requests ?? 0;
  }
  return props.historyData.failed_requests ?? 0;
});

const successfulRequests = computed(() => {
  if (isAllTime.value || !props.historyData) {
    return props.metrics?.successful_requests ?? 0;
  }
  const total = totalRequests.value;
  const failed = failedRequests.value;
  return Math.max(0, total - failed);
});

function formatTokensInt(n: number): string {
  if (n >= 1_000_000) return `${Math.round(n / 1_000_000)}M`;
  if (n >= 1_000) return `${Math.round(n / 1_000)}K`;
  return Math.round(n).toString();
}

const completionTokens = computed(() => {
  if (isAllTime.value || !props.historyData) {
    return props.metrics?.completion_tokens ?? 0;
  }
  return props.historyData.completion_tokens ?? 0;
});

const promptTokens = computed(() => {
  if (isAllTime.value || !props.historyData) {
    return props.metrics?.prompt_tokens ?? 0;
  }
  return props.historyData.prompt_tokens ?? 0;
});

const cachedTokens = computed(() => {
  if (isAllTime.value || !props.historyData) {
    return props.metrics?.cached_tokens ?? 0;
  }
  return props.historyData.cached_tokens ?? 0;
});

const totalTokens = computed(() => {
  if (isAllTime.value || !props.historyData) {
    return props.metrics?.total_tokens ?? 0;
  }
  return props.historyData.total_tokens ?? 0;
});

const cacheHitRate = computed(() => {
  const prompt = promptTokens.value;
  const cached = cachedTokens.value;
  if (prompt <= 0) return '0%';
  const rate = Math.min(100, Math.round((cached / prompt) * 100));
  return `${rate}%`;
});

const ttft = computed(() => {
  if (isAllTime.value) {
    const v = props.metrics?.stream?.avg_ttft_ms;
    return v !== undefined && v !== null && v > 0 ? `${Math.round(v)} ms` : '--';
  }
  // 选定周期下，优先展示后端在流式中按请求统计的真实平均 TTFT
  const histTtft = props.historyData?.avg_ttft_ms;
  if (histTtft !== undefined && histTtft !== null && histTtft > 0) {
    return `${Math.round(histTtft)} ms`;
  }
  // 若历史桶未单独记录 ttft（如老版本桶），优雅回退到全局 ttft
  const fallback = props.metrics?.stream?.avg_ttft_ms;
  return fallback !== undefined && fallback !== null && fallback > 0 ? `${Math.round(fallback)} ms` : '--';
});

const avgTps = computed(() => {
  if (isAllTime.value) {
    const v = props.metrics?.stream?.avg_tps;
    return v !== undefined && v !== null && v > 0 ? `${Math.round(v)} tok/s` : '--';
  }
  // 选定周期下，优先读取后端在该时间段内聚合的真实生成速率
  const histTps = props.historyData?.avg_tps;
  if (histTps !== undefined && histTps !== null && histTps > 0) {
    return `${Math.round(histTps)} tok/s`;
  }
  // 若无或无请求活跃，优雅回退到全局流式 tps
  const fallback = props.metrics?.stream?.avg_tps;
  return fallback !== undefined && fallback !== null && fallback > 0 ? `${Math.round(fallback)} tok/s` : '--';
});

const errorRate = computed(() => {
  const total = totalRequests.value;
  if (total === 0) return '0.0%';
  const failed = failedRequests.value;
  const rate = (failed / total) * 100;
  return `${rate.toFixed(1)}%`;
});
</script>

<template>
  <div class="mb-6">
    <!-- 头部周期选择切片 -->
    <div class="flex items-center justify-between mb-3.5">
      <div class="flex items-center gap-2">
        <span class="text-[13px] font-semibold text-slate-700">核心指标概览</span>
        <span class="text-xs text-slate-600">
          ({{ rangeOptions.find((o) => o.key === (range ?? '24h'))?.label }})
        </span>
      </div>
      <div class="segment-track inline-flex items-center bg-slate-200/60 p-0.5 rounded-lg border border-slate-300/40">
        <button
          v-for="opt in rangeOptions"
          :key="opt.key"
          type="button"
          class="px-3 py-1 text-xs font-medium rounded-md transition-all duration-150 cursor-pointer"
          :class="[
            (range ?? '24h') === opt.key
              ? 'bg-white text-slate-950 font-semibold shadow-2xs'
              : 'text-slate-600 hover:text-slate-900',
          ]"
          @click="emit('update:range', opt.key)"
        >
          {{ opt.label }}
        </button>
      </div>
    </div>

    <div
      class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-5 gap-4 transition-opacity duration-200"
      :class="{ 'opacity-60 pointer-events-none': loading }"
    >
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
          {{ isAllTime ? '累计平均首字延迟 (TTFT)' : '所选周期平均首字延迟 (TTFT)' }}
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
          {{ isAllTime ? '累计平均生成速率 (TPS)' : '所选周期平均生成速率 (TPS)' }}
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
          失败请求: <span class="font-mono text-slate-700">{{ failedRequests.toLocaleString() }} 次</span>
        </div>
      </div>
    </div>
  </div>
</template>
