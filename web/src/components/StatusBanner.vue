<script setup lang="ts">
import type { ConnectivityBarSeries } from '../types/telemetry';
import Icons from './ui/Icons.vue';
import UiButton from './ui/UiButton.vue';
import UptimeBars from './ui/UptimeBars.vue';

defineProps<{
  health: 'ok' | 'down' | 'degraded' | 'unknown';
  transport: 'sse' | 'polling' | 'offline';
  isDown: boolean;
  uptimeBars?: ConnectivityBarSeries;
}>();

const emit = defineEmits<{
  (e: 'retry'): void;
}>();
</script>

<template>
  <div
    class="flex flex-wrap items-center justify-between gap-4 px-5 py-3.5 rounded-xl mb-6 transition-all duration-200"
    :class="isDown ? 'bg-rose-50/90 text-rose-800' : 'borderless-card'"
  >
    <div class="flex flex-wrap items-center gap-4 text-sm">
      <!-- 极简脉冲呼吸灯 -->
      <span class="relative flex h-2.5 w-2.5 items-center justify-center">
        <span
          v-if="health === 'ok'"
          class="animate-ping absolute inline-flex h-full w-full rounded-full bg-emerald-400 opacity-75"
        />
        <span
          class="relative inline-flex rounded-full h-2 w-2"
          :class="{
            'bg-emerald-500': health === 'ok',
            'bg-amber-500': health === 'degraded',
            'bg-rose-500': health === 'down',
            'bg-slate-400': health === 'unknown',
          }"
        />
      </span>

      <span class="text-slate-700 font-semibold">
        网关状态: <strong class="font-bold text-slate-900">{{ health.toUpperCase() }}</strong>
      </span>

      <!-- 40根柱状连续排列连通性图例 (Uptime Bars) -->
      <div class="flex items-center pl-2 border-l border-slate-200">
        <UptimeBars
          :slots="uptimeBars?.slots"
          :latest-latency-ms="uptimeBars?.latest_latency_ms"
          bar-height="h-4.5"
        />
      </div>

      <span class="text-slate-300">·</span>

      <span
        class="px-2.5 py-0.5 rounded-full text-xs font-medium"
        :class="{
          'bg-emerald-50 text-emerald-800': transport === 'sse',
          'bg-slate-100 text-slate-700': transport === 'polling',
          'bg-rose-50 text-rose-700': transport === 'offline',
        }"
      >
        {{ transport === 'sse' ? '实时流 (SSE)' : transport === 'polling' ? '轮询中 (1.5s)' : '服务离线' }}
      </span>
    </div>

    <div v-if="isDown" class="flex items-center gap-2">
      <span class="text-xs text-rose-600 font-medium">连接异常</span>
      <UiButton
        variant="destructive"
        size="sm"
        @click="emit('retry')"
      >
        <Icons name="refresh" size="13" />
        重试连接
      </UiButton>
    </div>
  </div>
</template>
