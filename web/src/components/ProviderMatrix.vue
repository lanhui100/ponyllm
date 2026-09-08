<script setup lang="ts">
import type { ProviderFlowSnapshot } from '../types/telemetry';
import Icons from './ui/Icons.vue';
import UiBadge from './ui/UiBadge.vue';

defineProps<{
  providers: Record<string, ProviderFlowSnapshot> | undefined;
}>();
</script>

<template>
  <div class="bg-white rounded-xl shadow-xs p-5">
    <div class="flex items-center gap-2 text-xs font-semibold text-slate-800 mb-3">
      <Icons name="server" size="14" class="text-blue-600" />
      Provider 节点健康矩阵
    </div>

    <div v-if="!providers || Object.keys(providers).length === 0" class="text-xs text-slate-400 py-6 text-center">
      暂无活跃 Provider 节点数据
    </div>

    <div v-else class="overflow-x-auto">
      <table class="w-full text-left text-xs">
        <thead>
          <tr class="text-slate-400 border-b border-slate-100 font-medium">
            <th class="pb-2.5 font-medium">Provider</th>
            <th class="pb-2.5 font-medium">状态</th>
            <th class="pb-2.5 font-medium">流调用数</th>
            <th class="pb-2.5 font-medium">平均 TTFT</th>
            <th class="pb-2.5 font-medium">平均 TPS</th>
            <th class="pb-2.5 font-medium text-right">错误数</th>
          </tr>
        </thead>
        <tbody class="divide-y divide-slate-50">
          <tr v-for="(p, name) in providers" :key="name" class="hover:bg-slate-50/60 transition-colors">
            <td class="py-3 font-semibold text-slate-900">{{ name }}</td>
            <td class="py-3">
              <UiBadge :variant="(p.status === 'healthy' || !p.status) ? 'success' : p.status === 'degraded' ? 'warning' : 'destructive'">
                {{ (p.status || 'healthy').toUpperCase() }}
              </UiBadge>
            </td>
            <td class="py-3 text-slate-600 font-mono">{{ p.stream_count ?? 0 }}</td>
            <td class="py-3 text-slate-600 font-mono">
              {{ p.avg_ttft_ms !== undefined && p.avg_ttft_ms !== null ? `${p.avg_ttft_ms.toFixed(1)} ms` : '--' }}
            </td>
            <td class="py-3 text-slate-600 font-mono">
              {{ p.avg_tps !== undefined && p.avg_tps !== null ? `${p.avg_tps.toFixed(1)} tok/s` : '--' }}
            </td>
            <td class="py-3 text-right font-mono" :class="(p.error_count ?? 0) > 0 ? 'text-rose-600 font-semibold' : 'text-slate-400'">
              {{ p.error_count ?? 0 }}
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>
