<script setup lang="ts">
import { computed } from 'vue';
import type { ProviderFlowSnapshot } from '../types/telemetry';
import Icons from './ui/Icons.vue';
import UptimeBars from './ui/UptimeBars.vue';

const props = withDefaults(
  defineProps<{
    providers: Record<string, ProviderFlowSnapshot> | undefined;
    range?: '24h' | '7d' | '30d';
    providerTokens?: Record<string, number>;
  }>(),
  {
    range: '24h',
  }
);

const emit = defineEmits<{
  (e: 'update:range', range: '24h' | '7d' | '30d'): void;
}>();

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

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return n.toLocaleString();
}
</script>

<template>
  <div class="borderless-card p-6 mb-6 transition-all duration-200">
    <!-- 头部区域：标题与周期 Switch -->
    <div class="flex flex-wrap items-center justify-between gap-3 pb-4 mb-4 border-b border-slate-100">
      <div class="flex items-center gap-2.5">
        <div class="w-8 h-8 rounded-lg bg-orange-50 text-orange-600 flex items-center justify-center">
          <Icons name="server" size="16" />
        </div>
        <div>
          <h2 class="text-base font-bold text-slate-900 tracking-tight">提供商状态</h2>
          <p class="text-xs text-slate-400">活跃上游节点的连通性微柱切片与吞吐概览</p>
        </div>
      </div>

      <!-- 24小时 / 7天 / 30天 Switch 选择器 -->
      <div class="segment-track inline-flex items-center">
        <button
          v-for="opt in rangeOptions"
          :key="opt.key"
          type="button"
          class="px-3 py-1.5 text-xs font-semibold rounded-md transition-all duration-150 cursor-pointer"
          :class="[
            range === opt.key
              ? 'bg-white text-orange-600 shadow-xs'
              : 'text-slate-600 hover:text-slate-900',
          ]"
          @click="emit('update:range', opt.key)"
        >
          {{ opt.label }}
        </button>
      </div>
    </div>

    <div v-if="!providers || Object.keys(providers).length === 0" class="text-sm text-slate-400 py-8 text-center">
      暂无活跃 Provider 节点数据
    </div>

    <div v-else class="overflow-x-auto">
      <table class="w-full text-left text-sm">
        <thead>
          <tr class="text-slate-400 border-b border-slate-100 font-medium">
            <th class="pb-3 font-semibold whitespace-nowrap">提供商</th>
            <th class="pb-3 font-semibold whitespace-nowrap">连通性状态 (最近1分钟)</th>
            <th class="pb-3 font-semibold whitespace-nowrap">Token 总量</th>
            <th class="pb-3 font-semibold whitespace-nowrap">流调用数</th>
            <th class="pb-3 font-semibold whitespace-nowrap">平均 TTFT</th>
            <th class="pb-3 font-semibold whitespace-nowrap">平均 TPS</th>
            <th class="pb-3 font-semibold text-right whitespace-nowrap">错误数</th>
          </tr>
        </thead>
        <tbody class="divide-y divide-slate-50">
          <tr
            v-for="(p, name) in providers"
            :key="name"
            class="hover:bg-slate-50/70 transition-colors group"
          >
            <!-- Provider 名称 -->
            <td class="py-3.5 font-bold text-slate-900 whitespace-nowrap">
              <span class="px-2 py-0.5 rounded bg-slate-100/80 text-slate-800 font-mono text-xs">
                {{ name }}
              </span>
            </td>

            <!-- 柱状连续排列的连通性图例 (Uptime Bars) -->
            <td class="py-3.5 whitespace-nowrap">
              <UptimeBars
                :slots="p.uptime_bars?.slots"
                :latest-latency-ms="p.uptime_bars?.latest_latency_ms"
                bar-height="h-4"
              />
            </td>

            <!-- Token 总量与占比 -->
            <td class="py-3.5 whitespace-nowrap">
              <div class="flex items-baseline gap-2">
                <span class="font-mono font-bold text-slate-900">
                  {{ formatTokens(getProviderTokens(name, p)) }}
                </span>
                <span
                  v-if="totalAllTokens > 0"
                  class="text-[11px] font-mono text-slate-400"
                >
                  ({{ ((getProviderTokens(name, p) / totalAllTokens) * 100).toFixed(0) }}%)
                </span>
              </div>
            </td>

            <!-- 流调用数 -->
            <td class="py-3.5 text-slate-600 font-mono text-xs whitespace-nowrap">
              {{ p.stream_count ?? 0 }}
            </td>

            <!-- 平均 TTFT -->
            <td class="py-3.5 text-slate-600 font-mono text-xs whitespace-nowrap">
              {{ p.avg_ttft_ms !== undefined && p.avg_ttft_ms !== null ? `${p.avg_ttft_ms.toFixed(1)} ms` : '--' }}
            </td>

            <!-- 平均 TPS -->
            <td class="py-3.5 text-slate-600 font-mono text-xs whitespace-nowrap">
              {{ p.avg_tps !== undefined && p.avg_tps !== null ? `${p.avg_tps.toFixed(1)} tok/s` : '--' }}
            </td>

            <!-- 错误数 -->
            <td
              class="py-3.5 text-right font-mono text-xs font-semibold whitespace-nowrap"
              :class="(p.error_count ?? 0) > 0 ? 'text-rose-600' : 'text-slate-400'"
            >
              {{ p.error_count ?? 0 }}
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>
