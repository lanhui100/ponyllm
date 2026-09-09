<script setup lang="ts">
import type { ConnectivityBarSeries } from '../types/telemetry';
import Icons from './ui/Icons.vue';
import UiButton from './ui/UiButton.vue';
import UiTooltip from './ui/UiTooltip.vue';
import UptimeBars from './ui/UptimeBars.vue';

defineProps<{
  health: 'ok' | 'down' | 'degraded' | 'unknown';
  transport?: 'sse' | 'polling' | 'offline';
  isDown: boolean;
  uptimeBars?: ConnectivityBarSeries;
  speed24h?: number;
}>();

const emit = defineEmits<{
  (e: 'retry'): void;
}>();
</script>

<template>
  <div
    class="flex flex-wrap items-center justify-between gap-4 px-5 py-3.5 rounded-xl mb-6 transition-all duration-200"
    :class="isDown ? 'bg-rose-100 text-rose-800' : 'bg-white'"
  >
    <div class="flex flex-wrap items-center gap-4 text-[14px]">
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

      <div class="flex items-center gap-1.5">
        <span class="text-slate-700 font-medium">网关状态</span>
        <UiTooltip
          content="展示最近 2 分钟的网关探测微柱（5s/柱，共28柱：绿色畅通、黄色轻微延迟、红色异常），右侧展示最新耗时。"
          wrap
        >
          <button
            type="button"
            aria-label="网关状态说明"
            class="text-slate-400 hover:text-slate-600 transition-colors cursor-help inline-flex items-center"
          >
            <Icons name="info" size="14" />
          </button>
        </UiTooltip>
      </div>

      <!-- 28 柱状态图例 (Uptime Bars, 5s/柱, 最近2分钟) -->
      <div class="flex items-center pl-2 bg-slate-100/70 rounded-lg px-2 py-1">
        <UptimeBars
          :slots="uptimeBars?.slots"
          :slot-count="28"
          :latest-latency-ms="uptimeBars?.latest_latency_ms"
          :show-speed-24h="false"
          bar-height="h-4.5"
        />
      </div>
    </div>

    <div v-if="isDown" class="flex items-center gap-2">
      <span class="text-[13px] text-rose-600 font-medium">连接异常</span>
      <UiButton
        variant="destructive"
        size="sm"
        @click="emit('retry')"
      >
        <Icons name="refresh" size="14" />
        重试连接
      </UiButton>
    </div>
  </div>
</template>
