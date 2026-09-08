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
  }>(),
  {
    slots: () => [],
    slotCount: 40,
    showLatency: true,
    barHeight: 'h-5',
  }
);

const normalizedSlots = computed<ConnectivitySlot[]>(() => {
  const count = props.slotCount;
  const input = props.slots || [];
  if (input.length >= count) {
    return input.slice(-count);
  }
  const paddingCount = count - input.length;
  const padding: ConnectivitySlot[] = Array.from({ length: paddingCount }, (_, i) => ({
    timestamp_ms: Date.now() - (paddingCount - i) * 1500,
    status: 'empty',
  }));
  return [...padding, ...input];
});

const hasValidLatency = computed(() => {
  return typeof props.latestLatencyMs === 'number' && !isNaN(props.latestLatencyMs) && props.latestLatencyMs >= 0;
});

function formatTime(ts: number): string {
  const d = new Date(ts);
  return `${d.getHours().toString().padStart(2, '0')}:${d.getMinutes().toString().padStart(2, '0')}:${d.getSeconds().toString().padStart(2, '0')}`;
}

function getSlotTooltip(slot: ConnectivitySlot): string {
  const time = formatTime(slot.timestamp_ms);
  if (slot.status === 'empty') return `${time} · 无探测数据`;
  const lat = typeof slot.latency_ms === 'number' && !isNaN(slot.latency_ms) ? `${slot.latency_ms.toFixed(1)} ms` : '--';
  const statusLabel =
    slot.status === 'ok'
      ? '响应及时 (<300ms)'
      : slot.status === 'degraded'
      ? '响应一般 (300~1000ms)'
      : '响应超时/异常 (≥1000ms 或服务断开)';
  return `${time} · ${lat} · ${statusLabel}`;
}
</script>

<template>
  <div class="inline-flex items-center gap-2.5">
    <!-- 连续排列的微型状态柱 -->
    <div class="flex items-center gap-[2px]">
      <div
        v-for="(slot, idx) in normalizedSlots"
        :key="idx"
        data-testid="uptime-bar"
        :data-status="slot.status"
        :title="getSlotTooltip(slot)"
        class="w-[3px] rounded-[1px] transition-all duration-150 cursor-pointer"
        :class="[
          barHeight,
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
      class="text-xs font-mono font-medium px-2 py-0.5 rounded"
      :class="[
        !hasValidLatency
          ? 'text-slate-400 bg-slate-100'
          : latestLatencyMs! < 300
          ? 'text-emerald-700 bg-emerald-50'
          : latestLatencyMs! < 1000
          ? 'text-amber-700 bg-amber-50'
          : 'text-rose-700 bg-rose-50',
      ]"
    >
      {{ hasValidLatency ? `${latestLatencyMs!.toFixed(1)} ms` : '--' }}
    </div>
  </div>
</template>
