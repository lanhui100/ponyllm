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
  }>(),
  {
    slots: () => [],
    slotCount: 40,
    showLatency: true,
    barHeight: 'h-5',
    speed24h: undefined,
    showSpeed24h: true,
  }
);

const normalizedSlots = computed<ConnectivitySlot[]>(() => {
  const count = props.slotCount;
  const input = props.slots || [];
  if (input.length >= count) {
    return input.slice(-count);
  }
  const paddingCount = count - input.length;
  const stepMs = count <= 24 ? 5000 : 1500;
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

function formatTime(ts: number): string {
  const d = new Date(ts);
  return `${d.getHours().toString().padStart(2, '0')}:${d.getMinutes().toString().padStart(2, '0')}:${d.getSeconds().toString().padStart(2, '0')}`;
}

function getSlotTooltip(slot: ConnectivitySlot): string {
  const time = formatTime(slot.timestamp_ms);
  if (slot.status === 'empty') return `${time} · 无调用数据`;
  const lat = typeof slot.latency_ms === 'number' && !isNaN(slot.latency_ms) ? `${slot.latency_ms.toFixed(1)} ms` : '--';
  const speed = typeof slot.tps === 'number' && !isNaN(slot.tps) && slot.tps >= 0 ? ` · ${slot.tps.toFixed(1)} t/s` : '';
  const statusLabel =
    slot.status === 'ok'
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
          slotCount <= 10 ? 'w-2 rounded-[2px]' : 'w-1 rounded-[1px]',
          slot.status === 'ok'
            ? 'bg-emerald-500 hover:scale-y-125 hover:brightness-110'
            : slot.status === 'degraded'
            ? 'bg-amber-400 hover:scale-y-125 hover:brightness-110'
            : slot.status === 'down'
            ? 'bg-rose-500 hover:scale-y-125 hover:brightness-110'
            : 'bg-slate-200 hover:bg-slate-300',
        ]"
      />
    </div>

    <!-- 最新耗时指示 -->
    <div
      v-if="showLatency"
      data-testid="latest-latency"
      class="text-[13px] font-mono font-medium px-2.5 py-0.5 rounded-md"
      :class="[
        !hasValidLatency
          ? 'text-slate-500 bg-slate-200/70'
          : latestLatencyMs! < 300
          ? 'text-emerald-800 bg-emerald-100'
          : latestLatencyMs! < 1000
          ? 'text-amber-800 bg-amber-100'
          : 'text-rose-800 bg-rose-100',
      ]"
    >
      {{ hasValidLatency ? `${latestLatencyMs!.toFixed(1)} ms` : '--' }}
    </div>

    <!-- 24 小时平均速度指示 (t/s) -->
    <div
      v-if="showSpeed24h && speed24h !== undefined"
      data-testid="speed-24h"
      class="text-[13px] font-mono font-medium px-2.5 py-0.5 rounded-md text-sky-800 bg-sky-100 inline-flex items-center gap-1.5"
      :title="`24小时平均速度: ${hasValidSpeed24h ? speed24h!.toFixed(1) : '--'} t/s`"
    >
      <span class="text-xs text-sky-600 font-sans font-semibold">24h</span>
      <span>{{ hasValidSpeed24h ? `${speed24h!.toFixed(1)} t/s` : '-- t/s' }}</span>
    </div>
  </div>
</template>
