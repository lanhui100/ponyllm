<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted } from 'vue';
import Icons from '../ui/Icons.vue';

const props = defineProps<{
  startTime: string; // "HH:00" or "HH:mm"
  endTime: string;   // "HH:00" or "24:00"
}>();

const emit = defineEmits<{
  (e: 'update:startTime', val: string): void;
  (e: 'update:endTime', val: string): void;
}>();

const isOpen = ref(false);
const containerRef = ref<HTMLElement | null>(null);

// parse start and end hours (0 to 24)
function parseHour(val: string, isEnd = false): number {
  if (!val) return isEnd ? 24 : 0;
  const parts = val.split(':');
  const h = parseInt(parts[0], 10);
  return isNaN(h) ? (isEnd ? 24 : 0) : h;
}

const startH = computed(() => parseHour(props.startTime, false));
const endH = computed(() => parseHour(props.endTime, true));

// Selection phase: null (nothing clicked in this session) -> 'selecting-end' (picked start, waiting for end)
const selectionStart = ref<number | null>(null);
const hoverHour = ref<number | null>(null);

function formatHourLabel(h: number): string {
  return `${String(h).padStart(2, '0')}:00`;
}

function displayRange(): string {
  const s = props.startTime || '00:00';
  const e = props.endTime || '24:00';
  return `${s} ~ ${e}`;
}

function toggleDropdown() {
  isOpen.value = !isOpen.value;
  if (isOpen.value) {
    selectionStart.value = null;
    hoverHour.value = null;
  }
}

function handleHourClick(h: number) {
  if (selectionStart.value === null) {
    // 第一次点击：确定起始小时点
    selectionStart.value = h;
  } else {
    // 第二次点击：确定结束点（时段包含该小时，所以结束时间为 h+1:00，或者如果点击比起始小，则作为起始）
    const start = Math.min(selectionStart.value, h);
    const end = Math.max(selectionStart.value, h) + 1; // 跨度到该小时结束，例如选 8，结束是 09:00；选 8 到 23，结束是 24:00
    emit('update:startTime', `${String(start).padStart(2, '0')}:00`);
    emit('update:endTime', `${String(end).padStart(2, '0')}:00`);
    selectionStart.value = null;
    hoverHour.value = null;
    isOpen.value = false;
  }
}

function isHourSelected(h: number): boolean {
  if (selectionStart.value !== null) {
    if (hoverHour.value !== null) {
      const min = Math.min(selectionStart.value, hoverHour.value);
      const max = Math.max(selectionStart.value, hoverHour.value);
      return h >= min && h <= max;
    }
    return h === selectionStart.value;
  }
  return h >= startH.value && h < endH.value;
}

function isRangeStart(h: number): boolean {
  if (selectionStart.value !== null) {
    if (hoverHour.value !== null) {
      return h === Math.min(selectionStart.value, hoverHour.value);
    }
    return h === selectionStart.value;
  }
  return h === startH.value;
}

function isRangeEnd(h: number): boolean {
  if (selectionStart.value !== null) {
    if (hoverHour.value !== null) {
      return h === Math.max(selectionStart.value, hoverHour.value);
    }
    return h === selectionStart.value;
  }
  return h === endH.value - 1;
}

function handleClickOutside(event: MouseEvent) {
  if (containerRef.value && !containerRef.value.contains(event.target as Node)) {
    isOpen.value = false;
    selectionStart.value = null;
    hoverHour.value = null;
  }
}

onMounted(() => {
  document.addEventListener('click', handleClickOutside);
});

onUnmounted(() => {
  document.removeEventListener('click', handleClickOutside);
});
</script>

<template>
  <div ref="containerRef" class="relative inline-block text-xs">
    <!-- 触发按钮：显示当前整点范围 -->
    <button
      type="button"
      class="inline-flex items-center gap-1.5 px-2 py-1 bg-slate-50 hover:bg-slate-100/80 border border-slate-200 hover:border-slate-300 rounded font-mono text-slate-800 text-xs transition-colors cursor-pointer select-none"
      @click.stop="toggleDropdown"
    >
      <Icons name="activity" size="12" class="text-slate-500 shrink-0" />
      <span>{{ displayRange() }}</span>
      <Icons :name="isOpen ? 'chevron-down' : 'chevron-right'" size="10" class="text-slate-400 shrink-0" />
    </button>

    <!-- 0-23 整点时段选择浮层 -->
    <div
      v-if="isOpen"
      class="absolute left-0 top-full mt-1 z-50 p-2.5 bg-white/95 backdrop-blur-md rounded-xl border border-slate-200/90 shadow-xl w-64 select-none animate-in fade-in zoom-in-95 duration-100"
      @click.stop
    >
      <div class="flex items-center justify-between pb-2 mb-2 border-b border-slate-100 text-3xs text-slate-500">
        <span>
          {{ selectionStart === null ? '① 点击选择起始小时' : '② 点击选择结束小时' }}
        </span>
        <span v-if="selectionStart !== null" class="font-mono text-slate-900 font-semibold">
          从 {{ formatHourLabel(selectionStart) }}
        </span>
      </div>

      <!-- 24小时矩阵 (4列 x 6行) -->
      <div class="grid grid-cols-4 gap-1 text-center font-mono">
        <button
          v-for="h in 24"
          :key="`hour-${h - 1}`"
          type="button"
          class="py-1 px-0.5 rounded text-xs transition-all cursor-pointer relative"
          :class="[
            isHourSelected(h - 1)
              ? 'bg-slate-900 text-white font-semibold shadow-2xs'
              : 'text-slate-700 hover:bg-slate-100',
            isRangeStart(h - 1) ? 'ring-1 ring-white/50' : '',
            isRangeEnd(h - 1) ? 'ring-1 ring-white/50' : ''
          ]"
          @mouseenter="selectionStart !== null ? hoverHour = (h - 1) : null"
          @click="handleHourClick(h - 1)"
        >
          {{ String(h - 1).padStart(2, '0') }}:00
        </button>
      </div>

      <div class="mt-2 pt-2 border-t border-slate-100 flex items-center justify-between text-3xs text-slate-400">
        <span>范围: 00:00 ~ 24:00</span>
        <button
          type="button"
          class="text-slate-600 hover:text-slate-900 font-medium cursor-pointer"
          @click="isOpen = false"
        >
          完成
        </button>
      </div>
    </div>
  </div>
</template>
