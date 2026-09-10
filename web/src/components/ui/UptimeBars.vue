<script setup lang="ts">
import { computed } from 'vue';
import type { ConnectivitySlot } from '../../types/telemetry';

const props = withDefaults(
  defineProps<{
    slots?: ConnectivitySlot[];
    slotCount?: number;
    latestLatencyMs?: number;
    showLatency?: boolean;
    barHeight?: string;
    speed24h?: number;
    showSpeed24h?: boolean;
    isProvider?: boolean;
    flat?: boolean;
  }>(),
  {
    slots: () => [],
    slotCount: 40,
    showLatency: true,
    barHeight: 'h-5',
    speed24h: undefined,
    showSpeed24h: true,
    isProvider: false,
    flat: false,
  }
);

const normalizedSlots = computed<ConnectivitySlot[]>(() => {
  const count = props.slotCount;
  const input = props.slots || [];
  if (input.length >= count) {
    return input.slice(-count);
  }
  const paddingCount = count - input.length;
  const stepMs = count <= 24 ? 5000 : count <= 28 ? 5000 : 1500;
  const padding: ConnectivitySlot[] = Array.from({ length: paddingCount }, (_, i) => ({
    timestamp_ms: Date.now() - (paddingCount - i) * stepMs,
    status: 'empty',
  }));
  return [...padding, ...input];
});

const hasValidLatency = computed(() => {
  return typeof props.latestLatencyMs === 'number' && !isNaN(props.latestLatencyMs) && props.latestLatencyMs >= 0;
});

const hasValidSpeed24h = computed(() => {
  return typeof props.speed24h === 'number' && !isNaN(props.speed24h) && props.speed24h >= 0;
});

// 无背景（flat）模式：图例后面的指标只保留文字色，去掉胶囊背景与内边距
const latencyClasses = computed(() => {
  const thresholdColor = !hasValidLatency.value
    ? 'text-slate-500'
    : props.latestLatencyMs! < (props.isProvider ? 3000 : 300)
    ? 'text-emerald-800'
    : props.latestLatencyMs! < (props.isProvider ? 5000 : 1000)
    ? 'text-amber-800'
    : 'text-rose-800';
  const chip = props.flat ? '' : 'px-2.5 py-0.5 rounded-md';
  const bgColor = props.flat
    ? ''
    : !hasValidLatency.value
    ? 'bg-slate-200/70'
    : props.latestLatencyMs! < (props.isProvider ? 3000 : 300)
    ? 'bg-emerald-100'
    : props.latestLatencyMs! < (props.isProvider ? 5000 : 1000)
    ? 'bg-amber-100'
    : 'bg-rose-100';
  return [chip, thresholdColor, bgColor];
});

function formatTime(ts: number): string {
  const d = new Date(ts);
  return `${d.getHours().toString().padStart(2, '0')}:${d.getMinutes().toString().padStart(2, '0')}:${d.getSeconds().toString().padStart(2, '0')}`;
}

function getSlotTooltip(slot: ConnectivitySlot): string {
  const time = formatTime(slot.timestamp_ms);
  if (slot.status === 'empty') return `${time} · 无调用数据`;
  const lat = typeof slot.latency_ms === 'number' && !isNaN(slot.latency_ms) ? `${slot.latency_ms.toFixed(1)} ms` : '--';
  const speed = typeof slot.tps === 'number' && !isNaN(slot.tps) && slot.tps >= 0 ? ` · ${Math.round(slot.tps)} t/s` : '';
  const statusLabel = props.isProvider
    ? slot.status === 'ok'
      ? '响应及时 (<3s)'
      : slot.status === 'degraded'
      ? '响应一般 (3~5s)'
      : '响应超时/慢 (≥5s 或异常)'
    : slot.status === 'ok'
    ? '响应及时 (<300ms)'
    : slot.status === 'degraded'
    ? '响应一般 (300~1000ms)'
    : '响应超时/异常 (≥1000ms 或服务断开)';
  return `${time} · ${lat}${speed} · ${statusLabel}`;
}
</script>

<template>
  <div class="inline-flex items-center gap-2.5 shrink-0">
    <!-- 连续排列的微型状态柱 -->
    <div
      class="flex items-center flex-nowrap shrink-0"
      :class="slotCount <= 10 ? 'gap-1' : 'gap-[2px]'"
    >
      <div
        v-for="(slot, idx) in normalizedSlots"
        :key="idx"
        data-testid="uptime-bar"
        :data-status="slot.status"
        :title="getSlotTooltip(slot)"
        class="shrink-0 transition-all duration-150 cursor-pointer"
        :class="[
          barHeight,
          'w-1 rounded-[1px]',
          slot.status === 'ok'
            ? 'bg-emerald-500 hover:scale-y-125 hover:brightness-110'
            : slot.status === 'degraded'
            ? 'bg-amber-400 hover:scale-y-125 hover:brightness-110'
            : slot.status === 'down'
            ? 'bg-rose-500 hover:scale-y-125 hover:brightness-110'
            : 'bg-slate-400 hover:bg-slate-500',
        ]"
      />
    </div>

    <!-- 最新耗时指示 -->
    <div
      v-if="showLatency"
      data-testid="latest-latency"
      class="text-[13px] font-mono font-medium"
      :class="latencyClasses"
    >
      {{ hasValidLatency ? `${latestLatencyMs!.toFixed(1)} ms` : '--' }}
    </div>

    <!-- 24 小时平均速度指示 (t/s) -->
    <div
      v-if="showSpeed24h && speed24h !== undefined"
      data-testid="speed-24h"
      class="text-[13px] font-mono font-medium inline-flex items-center gap-1.5 text-sky-800"
      :class="!flat && 'px-2.5 py-0.5 rounded-md bg-sky-100'"
      :title="`24小时平均速度: ${hasValidSpeed24h ? Math.round(speed24h!) : '--'} t/s`"
    >
      <span class="text-xs text-sky-600 font-sans font-semibold">24h</span>
      <span>{{ hasValidSpeed24h ? `${Math.round(speed24h!)} t/s` : '-- t/s' }}</span>
    </div>
  </div>
</template>
