<script setup lang="ts">
import Icons from '../ui/Icons.vue';
import UiTooltip from '../ui/UiTooltip.vue';

const props = defineProps<{
  defaultEffort: string;
  maxEffort: string;
  disabled?: boolean;
}>();

const emit = defineEmits<{
  (e: 'update:defaultEffort', val: string): void;
  (e: 'update:maxEffort', val: string): void;
}>();

const TIERS = [
  { value: 'Off', label: '关闭 (Off)', desc: '不进行深度思考，响应最快、成本最低' },
  { value: 'Low', label: '轻度 (Low)', desc: '轻度推理，适合轻量逻辑和简单校验' },
  { value: 'Medium', label: '平衡 (Medium)', desc: '标准深度思考，兼顾质量与耗时' },
  { value: 'High', label: '深度 (High)', desc: '最大化认知推理预算，适合极高难度题目' },
];
</script>

<template>
  <div class="space-y-3 p-3 bg-slate-50/80 rounded-lg text-xs">
    <div class="flex items-center justify-between">
      <span class="font-medium text-slate-700 flex items-center gap-1.5">
        <Icons name="brain" size="14" class="text-indigo-500" />
        思考强度映射 (Thinking Effort)
        <UiTooltip content="设置模型思考深度的默认值与最大允许上限，防止调用超支">
          <Icons name="info" size="13" class="text-slate-400 hover:text-slate-600 cursor-pointer" />
        </UiTooltip>
      </span>
    </div>

    <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
      <!-- 默认思考深度 (基线) -->
      <div>
        <label class="block text-slate-500 mb-1 flex items-center gap-1">
          默认深度 (客户端未指定时使用)
          <UiTooltip content="客户端请求中若未声明 reasoning_effort 时所采用的默认档位">
            <Icons name="info" size="12" class="text-slate-400 cursor-pointer" />
          </UiTooltip>
        </label>
        <select
          :value="defaultEffort"
          :disabled="disabled"
          class="w-full bg-white border border-slate-200 rounded-md px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-1 focus:ring-blue-500"
          data-testid="thinking-default-select"
          @change="emit('update:defaultEffort', ($event.target as HTMLSelectElement).value)"
        >
          <option v-for="t in TIERS" :key="t.value" :value="t.value">
            {{ t.label }}
          </option>
        </select>
      </div>

      <!-- 最大允许上限 (天花板) -->
      <div>
        <label class="block text-slate-500 mb-1 flex items-center gap-1">
          最大上限 (强制拦截的天花板)
          <UiTooltip content="客户端请求所能达到的最高思考档位，超出将被自动钳位截断">
            <Icons name="info" size="12" class="text-slate-400 cursor-pointer" />
          </UiTooltip>
        </label>
        <select
          :value="maxEffort"
          :disabled="disabled"
          class="w-full bg-white border border-slate-200 rounded-md px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-1 focus:ring-blue-500"
          data-testid="thinking-max-select"
          @change="emit('update:maxEffort', ($event.target as HTMLSelectElement).value)"
        >
          <option v-for="t in TIERS" :key="t.value" :value="t.value">
            {{ t.label }}
          </option>
        </select>
      </div>
    </div>
  </div>
</template>
