<script setup lang="ts">
import { computed } from 'vue';
import type { ProviderFlowSnapshot } from '../types/telemetry';
import Icons from './ui/Icons.vue';
import UiTooltip from './ui/UiTooltip.vue';
import UptimeBars from './ui/UptimeBars.vue';

const props = withDefaults(
  defineProps<{
    providers: Record<string, ProviderFlowSnapshot> | undefined;
    range?: '24h' | '7d' | '30d';
    providerTokens?: Record<string, number>;
    providerPromptTokens?: Record<string, number>;
    providerCompletionTokens?: Record<string, number>;
    providerCachedTokens?: Record<string, number>;
  }>(),
  {
    range: '24h',
  }
);

const emit = defineEmits<{
  (e: 'update:range', range: '24h' | '7d' | '30d'): void;
}>();

const sortedProviders = computed(() => {
  if (!props.providers) return [];
  return Object.entries(props.providers)
    .map(([name, snapshot]) => ({ name, snapshot }))
    .sort((a, b) => a.name.localeCompare(b.name));
});

const rangeOptions: Array<{ key: '24h' | '7d' | '30d'; label: string }> = [
  { key: '24h', label: '24小时' },
  { key: '7d', label: '7天' },
  { key: '30d', label: '30天' },
];

const totalAllTokens = computed(() => {
  let sum = 0;
  if (props.providerTokens) {
    for (const v of Object.values(props.providerTokens)) {
      sum += v;
    }
  } else if (props.providers) {
    for (const p of Object.values(props.providers)) {
      sum += p.total_tokens || 0;
    }
  }
  return sum;
});

function getProviderTokens(name: string, p: ProviderFlowSnapshot): number {
  if (props.providerTokens && props.providerTokens[name] !== undefined) {
    return props.providerTokens[name];
  }
  return p.total_tokens || 0;
}

function getProviderPromptTokens(name: string, p: ProviderFlowSnapshot): number {
  if (props.providerPromptTokens && props.providerPromptTokens[name] !== undefined) {
    return props.providerPromptTokens[name];
  }
  return p.prompt_tokens || 0;
}

function getProviderCompletionTokens(name: string, p: ProviderFlowSnapshot): number {
  if (props.providerCompletionTokens && props.providerCompletionTokens[name] !== undefined) {
    return props.providerCompletionTokens[name];
  }
  return p.completion_tokens || 0;
}

function getProviderCachedTokens(name: string, p: ProviderFlowSnapshot): number {
  if (props.providerCachedTokens && props.providerCachedTokens[name] !== undefined) {
    return props.providerCachedTokens[name];
  }
  return p.cached_tokens || 0;
}

function getProviderCachedPercent(name: string, p: ProviderFlowSnapshot): string {
  const prompt = getProviderPromptTokens(name, p);
  const cached = getProviderCachedTokens(name, p);
  if (prompt + cached === 0) return '0%';
  const rate = Math.round((cached / (prompt + cached)) * 100);
  return `${rate}%`;
}

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${Math.round(n / 1_000_000)}M`;
  if (n >= 1_000) return `${Math.round(n / 1_000)}K`;
  return Math.round(n).toString();
}

function getProviderTtft(p: any): string {
  const val = p.avg_ttft_ms ?? p.ttft_ms;
  const hasCalls = (p.stream_count ?? 0) > 0 || (p.total_requests ?? 0) > 0;
  if (!hasCalls || val === undefined || val === null || val <= 0) return '--';
  return `${Math.round(val)} ms`;
}

function getProviderTps(p: any): string {
  const val = p.avg_tps ?? p.tps;
  const hasCalls = (p.stream_count ?? 0) > 0 || (p.total_requests ?? 0) > 0;
  if (!hasCalls || val === undefined || val === null || val <= 0) return '--';
  return `${Math.round(val)} tok/s`;
}
</script>

<template>
  <div class="swiss-card bg-white/45 backdrop-blur-xs p-6 mb-6 transition-all duration-200 border border-white/40">
    <!-- 头部区域：标题与周期 Switch -->
    <div class="flex flex-wrap items-center justify-between gap-3 pb-4 mb-4 bg-white/35 rounded-lg px-3 pt-3">
      <div class="flex items-center gap-2.5">
        <div class="w-8 h-8 rounded-lg bg-orange-100 text-orange-600 flex items-center justify-center">
          <Icons name="server" size="16" />
        </div>
        <div>
          <div class="flex items-center gap-2">
            <h2 class="text-base font-bold text-slate-900 tracking-tight">提供商状态</h2>
            <UiTooltip
              content="展示各上游供应商最近调用的连通性微柱（每柱一次调用，最近40次，绿色畅通、黄色延迟、红色异常）、统计周期内的累计 Token 消耗与占比，以及平均首字延迟（TTFT）和平均生成速率（TPS）。数据经服务端持久化，重启后可恢复。"
              wrap
            >
              <button
                type="button"
                aria-label="提供商状态说明"
                class="text-slate-400 hover:text-slate-600 transition-colors cursor-help inline-flex items-center"
              >
                <Icons name="info" size="14" />
              </button>
            </UiTooltip>
          </div>
          <p class="text-[13px] text-slate-500">活跃上游节点的连通性微柱切片与吞吐概览</p>
        </div>
      </div>

      <!-- 24小时 / 7天 / 30天 Switch 选择器 -->
      <div class="segment-track inline-flex items-center">
          <button
            v-for="opt in rangeOptions"
            :key="opt.key"
            type="button"
            class="px-3.5 py-1.5 text-[13px] font-medium rounded-md transition-all duration-150 cursor-pointer"
            :class="[
              range === opt.key
                ? 'bg-white text-slate-950 font-semibold'
                : 'text-slate-600 hover:text-slate-900',
            ]"
          @click="emit('update:range', opt.key)"
        >
          {{ opt.label }}
        </button>
      </div>
    </div>

    <div v-if="!providers || Object.keys(providers).length === 0" class="text-sm text-slate-500 py-8 text-center">
      暂无活跃 Provider 节点数据
    </div>

    <div v-else class="overflow-x-auto">
      <table class="w-full text-left text-[14px]">
        <thead>
          <tr class="text-slate-500 bg-white/60 font-medium text-[13px]">
            <th class="pb-3 pt-2 px-2 font-semibold whitespace-nowrap rounded-l-lg">提供商</th>
            <th class="pb-3 pt-2 font-semibold whitespace-nowrap">连通性状态 (最近调用)</th>
            <th class="pb-3 pt-2 font-semibold whitespace-nowrap text-slate-700">输入 Token</th>
            <th class="pb-3 pt-2 font-semibold whitespace-nowrap text-sky-700">输出 Token</th>
            <th class="pb-3 pt-2 font-semibold whitespace-nowrap text-emerald-700">缓存命中</th>
            <th class="pb-3 pt-2 font-semibold whitespace-nowrap">流调用数</th>
            <th class="pb-3 pt-2 font-semibold whitespace-nowrap">平均 TTFT</th>
            <th class="pb-3 pt-2 font-semibold whitespace-nowrap">平均 TPS</th>
            <th class="pb-3 pt-2 px-2 font-semibold text-right whitespace-nowrap rounded-r-lg">错误数</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="{ name, snapshot: p } in sortedProviders"
            :key="name"
            class="hover:bg-white/70 transition-colors group"
          >
            <!-- Provider 名称 -->
            <td class="py-3.5 px-2 font-bold text-slate-900 whitespace-nowrap">
              <span class="px-2.5 py-1 rounded-md bg-slate-200/60 text-slate-900 font-mono text-[13px] font-semibold">
                {{ name }}
              </span>
            </td>

            <!-- 柱状连续排列的连通性图例 (Uptime Bars, 3s/5s 阈值) -->
            <td class="py-3.5 whitespace-nowrap">
              <UptimeBars
                :slots="p.uptime_bars?.slots"
                :latest-latency-ms="p.uptime_bars?.latest_latency_ms"
                :is-provider="true"
                bar-height="h-4"
              />
            </td>

            <!-- 输入 Token -->
            <td class="py-3.5 whitespace-nowrap">
              <span class="font-mono font-semibold text-slate-800">
                {{ formatTokens(getProviderPromptTokens(name, p)) }}
              </span>
            </td>

            <!-- 输出 Token -->
            <td class="py-3.5 whitespace-nowrap">
              <span class="font-mono font-bold text-sky-700">
                {{ formatTokens(getProviderCompletionTokens(name, p)) }}
              </span>
            </td>

            <!-- 缓存命中 -->
            <td class="py-3.5 whitespace-nowrap">
              <div class="flex items-baseline gap-1.5">
                <span class="font-mono font-semibold text-emerald-700">
                  {{ formatTokens(getProviderCachedTokens(name, p)) }}
                </span>
                <span
                  v-if="getProviderCachedTokens(name, p) > 0"
                  class="text-xs font-mono font-bold text-emerald-600"
                >
                  ({{ getProviderCachedPercent(name, p) }})
                </span>
              </div>
            </td>

            <!-- 流调用数 -->
            <td class="py-3.5 text-slate-700 font-mono text-[13px] whitespace-nowrap">
              {{ p.stream_count ?? 0 }}
            </td>

            <!-- 平均 TTFT -->
            <td class="py-3.5 text-slate-700 font-mono text-[13px] whitespace-nowrap">
              {{ getProviderTtft(p) }}
            </td>

            <!-- 平均 TPS -->
            <td class="py-3.5 text-slate-700 font-mono text-[13px] whitespace-nowrap">
              {{ getProviderTps(p) }}
            </td>

            <!-- 错误数 -->
            <td
              class="py-3.5 text-right font-mono text-[13px] font-semibold whitespace-nowrap"
              :class="(p.error_count ?? 0) > 0 ? 'text-rose-600' : 'text-slate-500'"
            >
              {{ p.error_count ?? 0 }}
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>
