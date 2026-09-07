<script setup lang="ts">
import { ref, watch } from 'vue';

const props = defineProps<{
  currentStrategy: string;
  adminWriteEnabled: boolean;
}>();

const emit = defineEmits<{
  (e: 'update', strategy: string): Promise<void>;
}>();

const selected = ref(props.currentStrategy || 'economy');
const saving = ref(false);

watch(() => props.currentStrategy, (val) => {
  if (val) selected.value = val;
});

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
  <div class="section-container">
    <div class="section-header">
      <div>
        <h2 class="section-title">全局路由调度策略 (Global Strategy)</h2>
        <p class="section-desc">决定网关派发请求时跨 Provider 与跨模型的调度偏好算法</p>
      </div>
      <div v-if="saving" class="saving-indicator">保存中...</div>
    </div>

    <div class="strategy-grid">
      <div
        v-for="s in strategies"
        :key="s.id"
        class="strategy-card"
        :class="{
          active: selected === s.id,
          disabled: !adminWriteEnabled,
        }"
        data-testid="strategy-card"
        @click="handleSelect(s.id)"
      >
        <div class="card-header">
          <span class="card-title">{{ s.title }}</span>
          <span class="card-tag">{{ s.tag }}</span>
        </div>
        <p class="card-desc">{{ s.desc }}</p>
        <div class="card-footer">
          <span v-if="selected === s.id" class="active-badge">✓ 当前生效</span>
          <span v-else class="inactive-badge">点击切换</span>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.section-container {
  background: #ffffff;
  border: 1px solid #e2e8f0;
  border-radius: 8px;
  padding: 20px;
}

.section-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 20px;
}

.section-title {
  font-size: 18px;
  font-weight: 700;
  color: #0f172a;
  margin: 0 0 4px 0;
}

.section-desc {
  font-size: 13px;
  color: #64748b;
  margin: 0;
}

.saving-indicator {
  font-size: 13px;
  font-weight: 600;
  color: #2563eb;
}

.strategy-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
  gap: 16px;
}

.strategy-card {
  border: 2px solid #e2e8f0;
  border-radius: 8px;
  padding: 16px;
  cursor: pointer;
  transition: all 0.2s ease;
  display: flex;
  flex-direction: column;
  justify-content: space-between;
  background: #f8fafc;
}

.strategy-card:hover:not(.disabled) {
  border-color: #93c5fd;
  background: #ffffff;
}

.strategy-card.active {
  border-color: #2563eb;
  background: #eff6ff;
}

.strategy-card.disabled {
  cursor: not-allowed;
  opacity: 0.7;
}

.card-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 8px;
}

.card-title {
  font-size: 14px;
  font-weight: 700;
  color: #0f172a;
}

.card-tag {
  font-size: 11px;
  padding: 2px 6px;
  background: #e2e8f0;
  color: #475569;
  border-radius: 4px;
  font-weight: 600;
}

.strategy-card.active .card-tag {
  background: #dbeafe;
  color: #1d4ed8;
}

.card-desc {
  font-size: 12px;
  color: #64748b;
  line-height: 1.5;
  margin: 0 0 16px 0;
  flex: 1;
}

.card-footer {
  display: flex;
  justify-content: flex-end;
}

.active-badge {
  font-size: 12px;
  font-weight: 700;
  color: #2563eb;
}

.inactive-badge {
  font-size: 12px;
  color: #94a3b8;
}
</style>
