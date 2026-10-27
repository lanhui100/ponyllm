<script setup lang="ts">
import { ref, watch, computed } from 'vue';
import Icons from '../ui/Icons.vue';
import UiTooltip from '../ui/UiTooltip.vue';

const ALL_LEVELS = [
  { value: 'off', label: 'Off (关闭)', placeholder: '留空表示不发送参数' },
  { value: 'low', label: 'Low (轻度)', placeholder: '默认: low' },
  { value: 'medium', label: 'Medium (平衡)', placeholder: '默认: medium' },
  { value: 'high', label: 'High (深度)', placeholder: '默认: high' },
  { value: 'max', label: 'Max (极限)', placeholder: '默认: max' },
] as const;

const props = withDefaults(
  defineProps<{
    modelValue?: Record<string, string> | null;
    disabled?: boolean;
  }>(),
  {
    modelValue: null,
    disabled: false,
  }
);

const emit = defineEmits<{
  (e: 'update:modelValue', val: Record<string, string> | null): void;
}>();

const isCustomEnabled = ref(false);

interface EffortRow {
  level: string;
  wire: string;
}

const rows = ref<EffortRow[]>([]);

function syncFromProps() {
  if (props.modelValue && typeof props.modelValue === 'object') {
    isCustomEnabled.value = true;
    rows.value = Object.entries(props.modelValue).map(([level, wire]) => ({
      level: level.toLowerCase(),
      wire: wire ?? '',
    }));
  } else {
    isCustomEnabled.value = false;
    rows.value = [
      { level: 'off', wire: '' },
      { level: 'low', wire: 'low' },
      { level: 'medium', wire: 'medium' },
      { level: 'high', wire: 'high' },
      { level: 'max', wire: 'max' },
    ];
  }
}

watch(() => props.modelValue, syncFromProps, { immediate: true });

function emitUpdate() {
  if (!isCustomEnabled.value) {
    emit('update:modelValue', null);
    return;
  }
  const result: Record<string, string> = {};
  for (const r of rows.value) {
    if (r.level) {
      result[r.level] = r.wire;
    }
  }
  emit('update:modelValue', Object.keys(result).length > 0 ? result : null);
}

function toggleCustom(enabled: boolean) {
  if (props.disabled) return;
  isCustomEnabled.value = enabled;
  if (enabled && rows.value.length === 0) {
    rows.value = [
      { level: 'off', wire: '' },
      { level: 'low', wire: 'low' },
      { level: 'medium', wire: 'medium' },
      { level: 'high', wire: 'high' },
      { level: 'max', wire: 'max' },
    ];
  }
  emitUpdate();
}

const unusedLevels = computed(() => {
  const used = new Set(rows.value.map((r) => r.level));
  return ALL_LEVELS.filter((l) => !used.has(l.value));
});

function addRow() {
  if (props.disabled || unusedLevels.value.length === 0) return;
  const next = unusedLevels.value[0];
  rows.value.push({
    level: next.value,
    wire: next.value === 'off' ? '' : next.value,
  });
  emitUpdate();
}

function removeRow(index: number) {
  if (props.disabled) return;
  rows.value.splice(index, 1);
  emitUpdate();
}

function onRowChange() {
  emitUpdate();
}
</script>

<template>
  <div class="space-y-2" data-testid="thinking-effort-map-editor">
    <div class="flex items-center justify-between">
      <span class="text-xs font-medium text-slate-700 flex items-center gap-1">
        <span>上游思考字符映射</span>
        <UiTooltip content="针对不同模型提供商自定义思考强度上游参数（如将 high 映射为 xhigh）。Off 留空代表不发参数。">
          <Icons name="info" size="12" class="text-slate-400 cursor-pointer" />
        </UiTooltip>
      </span>
      <button
        type="button"
        :disabled="disabled"
        class="text-xs px-2 py-0.5 rounded border transition-colors cursor-pointer"
        :class="[
          isCustomEnabled
            ? 'bg-slate-900 text-white border-slate-900 font-medium'
            : 'bg-white text-slate-600 border-slate-200 hover:bg-slate-50'
        ]"
        data-testid="toggle-custom-effort-map-btn"
        @click="toggleCustom(!isCustomEnabled)"
      >
        {{ isCustomEnabled ? '自定义中' : '默认映射' }}
      </button>
    </div>

    <!-- 映射行列表 -->
    <div v-if="isCustomEnabled" class="p-2.5 bg-slate-50 rounded-lg border border-slate-200/80 space-y-2">
      <div
        v-for="(row, idx) in rows"
        :key="idx"
        class="flex items-center gap-2"
        data-testid="effort-map-row"
      >
        <select
          v-model="row.level"
          :disabled="disabled"
          class="w-32 bg-white border border-slate-200/80 rounded px-2 py-1 text-xs text-slate-700 focus:outline-none focus:ring-1 focus:ring-slate-400 font-mono"
          data-testid="effort-map-level-select"
          @change="onRowChange"
        >
          <option
            v-for="lvl in ALL_LEVELS"
            :key="lvl.value"
            :value="lvl.value"
            :disabled="lvl.value !== row.level && rows.some(r => r.level === lvl.value)"
          >
            {{ lvl.value.toUpperCase() }}
          </option>
        </select>

        <span class="text-slate-400 text-xs">➔</span>

        <input
          v-model="row.wire"
          type="text"
          :disabled="disabled"
          :placeholder="ALL_LEVELS.find(l => l.value === row.level)?.placeholder || 'wire string'"
          class="flex-1 bg-white border border-slate-200/80 rounded px-2 py-1 text-xs text-slate-800 focus:outline-none focus:ring-1 focus:ring-slate-400 font-mono"
          data-testid="effort-map-wire-input"
          @input="onRowChange"
        />

        <button
          type="button"
          :disabled="disabled"
          class="p-1 text-slate-400 hover:text-rose-600 rounded hover:bg-rose-50 transition-colors cursor-pointer"
          data-testid="effort-map-delete-btn"
          title="删除该档位"
          @click="removeRow(idx)"
        >
          <Icons name="trash" size="13" />
        </button>
      </div>

      <div class="flex items-center justify-between pt-1">
        <span class="text-[11px] text-slate-400">
          未列出的档位将自动回落或被禁用；Off 留空代表向协议省略该字段。
        </span>
        <button
          v-if="unusedLevels.length > 0"
          type="button"
          :disabled="disabled"
          class="text-xs text-slate-600 hover:text-slate-900 font-medium flex items-center gap-1 px-1.5 py-0.5 rounded hover:bg-slate-200/60 transition-colors cursor-pointer"
          data-testid="effort-map-add-btn"
          @click="addRow"
        >
          <Icons name="plus" size="12" />
          <span>添加档位</span>
        </button>
      </div>
    </div>
  </div>
</template>
