<script setup lang="ts">
import { ref, watch } from 'vue';
import Icons from '../ui/Icons.vue';
import UiBadge from '../ui/UiBadge.vue';

const props = defineProps<{
  currentStrategy: string;
  adminWriteEnabled: boolean;
}>();

const emit = defineEmits<{
  (e: 'update', strategy: string): Promise<void>;
}>();

const selected = ref(props.currentStrategy || 'economy');
const saving = ref(false);

watch(
  () => props.currentStrategy,
  (val) => {
    if (val) selected.value = val;
  }
);

const strategies = [
  {
    id: 'economy',
    title: 'Economy 经济优先',
    desc: '优先选择单价最低的 Provider 与模型，按输入/输出价格自动排序，适合离线批处理与成本敏感场景。',
    tag: '成本最优',
  },
  {
    id: 'speed',
    title: 'Speed 速度优先',
    desc: '基于历史滑动窗口 TTFT (首字延迟) 与 TPS 动态选路，优先调度响应最快的实例。',
    tag: '极致响应',
  },
  {
    id: 'reliable',
    title: 'Reliable 稳定优先',
    desc: '以失败率最低与成功率最高为第一准则，发生限流/报错时以最短冷却重试备用节点。',
    tag: '高可用保障',
  },
  {
    id: 'balanced',
    title: 'Balanced 综合均衡',
    desc: '综合考虑价格、延迟与成功率三个维度的加权评分，兼顾成本与体验，适合通用业务流量。',
    tag: '推荐生产',
  },
];

async function handleSelect(id: string) {
  if (!props.adminWriteEnabled || saving.value || selected.value === id) return;
  selected.value = id;
  saving.value = true;
  try {
    await emit('update', id);
  } catch (err: unknown) {
    alert(`切换策略失败: ${err instanceof Error ? err.message : String(err)}`);
    selected.value = props.currentStrategy;
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <div class="swiss-card p-6">
    <div class="flex items-center justify-between mb-5">
      <div>
        <h2 class="text-base font-bold text-slate-900 flex items-center gap-2">
          <Icons name="activity" size="18" class="text-indigo-600" />
          全局分流调度策略
        </h2>
        <p class="text-xs text-slate-500 mt-1">
          决定网关向模型与服务商路由请求时的全局偏好算法
        </p>
      </div>
      <div v-if="saving" class="text-xs font-semibold text-indigo-600 animate-pulse">
        保存生效中...
      </div>
    </div>

    <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
      <div
        v-for="s in strategies"
        :key="s.id"
        class="p-4.5 rounded-xl transition-all duration-200 cursor-pointer select-none flex flex-col justify-between border"
        :class="[
          selected === s.id
            ? 'bg-indigo-50/80 border-indigo-400 ring-2 ring-indigo-500/50 shadow-xs'
            : 'bg-white/40 border-white/50 backdrop-blur-xs hover:bg-white/60 hover:shadow-xs',
          { 'opacity-60 cursor-not-allowed': !adminWriteEnabled },
        ]"
        data-testid="strategy-card"
        @click="handleSelect(s.id)"
      >
        <div>
          <div class="flex items-center justify-between mb-2">
            <span class="font-bold text-sm text-slate-900">{{ s.title }}</span>
            <UiBadge :variant="selected === s.id ? 'default' : 'secondary'">
              {{ s.tag }}
            </UiBadge>
          </div>
          <p class="text-xs text-slate-500 leading-relaxed">
            {{ s.desc }}
          </p>
        </div>

        <div class="mt-4 flex items-center justify-between text-xs pt-2 border-t border-slate-200/50">
          <span class="text-slate-500">状态</span>
          <span
            class="font-semibold flex items-center gap-1"
            :class="selected === s.id ? 'text-indigo-600' : 'text-slate-500'"
          >
            <Icons v-if="selected === s.id" name="check" size="14" />
            {{ selected === s.id ? '当前生效' : '未激活' }}
          </span>
        </div>
      </div>
    </div>
  </div>
</template>
