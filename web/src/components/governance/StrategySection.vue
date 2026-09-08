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
  <div class="bg-white rounded-xl shadow-xs p-5">
    <div class="flex items-center justify-between mb-4">
      <div>
        <h2 class="text-sm font-bold text-slate-900 flex items-center gap-1.5">
          <Icons name="activity" size="16" class="text-blue-600" />
          全局分流调度策略
        </h2>
        <p class="text-xs text-slate-400 mt-0.5">
          决定网关向模型与服务商路由请求时的全局偏好算法
        </p>
      </div>
      <div v-if="saving" class="text-xs font-semibold text-blue-600 animate-pulse">
        保存生效中...
      </div>
    </div>

    <div class="grid grid-cols-1 sm:grid-cols-2 gap-3.5">
      <div
        v-for="s in strategies"
        :key="s.id"
        class="p-4 rounded-xl border transition-all duration-200 cursor-pointer select-none flex flex-col justify-between"
        :class="[
          selected === s.id
            ? 'bg-blue-50/50 border-blue-500/80 shadow-2xs'
            : 'bg-slate-50/60 border-slate-200/70 hover:bg-white hover:border-slate-300',
          { 'opacity-60 cursor-not-allowed': !adminWriteEnabled },
        ]"
        data-testid="strategy-card"
        @click="handleSelect(s.id)"
      >
        <div>
          <div class="flex items-center justify-between mb-2">
            <span class="font-bold text-xs text-slate-900">{{ s.title }}</span>
            <UiBadge :variant="selected === s.id ? 'default' : 'secondary'">
              {{ s.tag }}
            </UiBadge>
          </div>
          <p class="text-xs text-slate-500 leading-relaxed mb-4">
            {{ s.desc }}
          </p>
        </div>

        <div class="flex items-center justify-end text-xs font-medium">
          <span v-if="selected === s.id" class="text-blue-600 flex items-center gap-1">
            <Icons name="check" size="13" />
            当前生效
          </span>
          <span v-else class="text-slate-400">点击应用</span>
        </div>
      </div>
    </div>
  </div>
</template>
