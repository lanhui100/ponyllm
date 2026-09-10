<script setup lang="ts">
import Icons from '../ui/Icons.vue';
import UiTooltip from '../ui/UiTooltip.vue';

const props = withDefaults(
  defineProps<{
    defaultEffort: string;
    maxEffort?: string;
    disabled?: boolean;
  }>(),
  {
    maxEffort: 'High',
    disabled: false,
  }
);

const emit = defineEmits<{
  (e: 'update:defaultEffort', val: string): void;
  (e: 'update:maxEffort', val: string): void;
}>();

const TIERS = [
  { value: 'Off', label: '关闭', sub: 'Off', desc: '不进行深度推理思考，响应最快、极速省流' },
  { value: 'Low', label: '轻度', sub: 'Low', desc: '轻度推理，适合轻量逻辑和简单步骤校验' },
  { value: 'Medium', label: '平衡', sub: 'Medium', desc: '标准深度思考，兼顾推理质量与响应速度' },
  { value: 'High', label: '深度', sub: 'High', desc: '最大化认知推理预算，适合极高难度复杂任务' },
];

function selectEffort(val: string) {
  if (props.disabled) return;
  emit('update:defaultEffort', val);
  emit('update:maxEffort', val === 'Off' ? 'Off' : 'High');
}
</script>

<template>
  <div class="space-y-2 text-sm">
    <div class="flex items-center justify-between">
      <label class="font-medium text-slate-800 flex items-center gap-1.5 text-[13px]">
        <Icons name="brain" size="14" class="text-slate-700" />
        思考强度 (Thinking Effort)
        <UiTooltip content="设置模型推理思考深度的预设档位，按需开启深度认知">
          <Icons name="info" size="13" class="text-slate-400 hover:text-slate-600 cursor-pointer" />
        </UiTooltip>
      </label>
      <span class="text-slate-500 text-xs font-mono">当前: {{ defaultEffort || 'Off' }}</span>
    </div>

    <!-- 纯按钮分段选项器 (无最大上限) -->
    <div
      class="grid grid-cols-4 gap-1.5 p-1 bg-slate-100/90 rounded-lg border border-slate-200/70 select-none"
      data-testid="thinking-effort-segment"
    >
      <UiTooltip
        v-for="t in TIERS"
        :key="t.value"
        :content="t.desc"
      >
        <button
          type="button"
          :disabled="disabled"
          :data-testid="`thinking-btn-${t.value.toLowerCase()}`"
          class="w-full py-2 px-1 rounded-md text-[13px] font-medium transition-all duration-150 flex flex-col items-center justify-center gap-0.5 cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed"
          :class="[
            defaultEffort === t.value
              ? 'bg-slate-900 text-white shadow-2xs font-semibold'
              : 'text-slate-600 hover:text-slate-900 hover:bg-white/50'
          ]"
          @click="selectEffort(t.value)"
        >
          <span>{{ t.label }}</span>
          <span
            class="text-xs font-mono"
            :class="defaultEffort === t.value ? 'text-slate-300 font-semibold' : 'text-slate-500'"
          >
            {{ t.sub }}
          </span>
        </button>
      </UiTooltip>
    </div>
  </div>
</template>
