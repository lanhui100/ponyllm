<script setup lang="ts">
import { ref, watch } from 'vue';
import Icons from '../ui/Icons.vue';
import { toast } from '../../composables/useToast';
import UiBadge from '../ui/UiBadge.vue';
import UiButton from '../ui/UiButton.vue';

const props = defineProps<{
  currentStrategy: string;
  adminWriteEnabled: boolean;
  autoModels?: string[];
  activeModelsOrder?: string[];
}>();

const emit = defineEmits<{
  (e: 'update', strategy: string): Promise<void>;
  (e: 'updateAutoModels', models: string[]): Promise<void>;
}>();

const selected = ref(props.currentStrategy || 'economy');
const saving = ref(false);

const localAutoModels = ref<string[]>([...(props.autoModels || [])]);
const newModelInput = ref('');
const savingAutoModels = ref(false);

watch(
  () => props.currentStrategy,
  (val) => {
    if (val) selected.value = val;
  }
);

watch(
  () => props.autoModels,
  (val) => {
    if (val) localAutoModels.value = [...val];
  },
  { deep: true }
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
    toast.success('全局调度策略已生效');
  } catch (err: unknown) {
    toast.error(`切换策略失败: ${err instanceof Error ? err.message : String(err)}`);
    selected.value = props.currentStrategy;
  } finally {
    saving.value = false;
  }
}

function moveUp(index: number) {
  if (index <= 0) return;
  const list = [...localAutoModels.value];
  const item = list.splice(index, 1)[0];
  list.splice(index - 1, 0, item);
  localAutoModels.value = list;
}

function moveDown(index: number) {
  if (index >= localAutoModels.value.length - 1) return;
  const list = [...localAutoModels.value];
  const item = list.splice(index, 1)[0];
  list.splice(index + 1, 0, item);
  localAutoModels.value = list;
}

function removeModel(index: number) {
  const list = [...localAutoModels.value];
  list.splice(index, 1);
  localAutoModels.value = list;
}

function addModel() {
  const trimmed = newModelInput.value.trim();
  if (!trimmed) return;
  if (!localAutoModels.value.includes(trimmed)) {
    localAutoModels.value.push(trimmed);
  }
  newModelInput.value = '';
}

async function handleSaveAutoModels() {
  if (!props.adminWriteEnabled || savingAutoModels.value) return;
  savingAutoModels.value = true;
  try {
    await emit('updateAutoModels', localAutoModels.value);
    toast.success('Auto 候选模型优先级排序已保存生效');
  } catch (err: unknown) {
    toast.error(`保存 Auto 模型失败: ${err instanceof Error ? err.message : String(err)}`);
  } finally {
    savingAutoModels.value = false;
  }
}
</script>

<template>
  <div class="space-y-6">
    <!-- 全局调度策略卡片 -->
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

    <!-- 纯 Auto 候选模型与优先级排序卡片 -->
    <div class="swiss-card p-6">
      <div class="flex items-center justify-between mb-4">
        <div>
          <h2 class="text-base font-bold text-slate-900 flex items-center gap-2">
            <Icons name="sparkles" size="18" class="text-emerald-600" />
            Auto 智能路由优先级与高可用候选
          </h2>
          <p class="text-xs text-slate-500 mt-1">
            下游 Agent 调用纯 <code class="px-1 py-0.5 bg-slate-100 rounded text-slate-800 font-mono">auto</code> 时的选型规则：首选免费主力，次选收费主力；顺序从上到下逐级备选与故障熔断
          </p>
        </div>
        <div class="flex items-center gap-2">
          <UiButton
            variant="default"
            size="sm"
            :disabled="!adminWriteEnabled || savingAutoModels"
            @click="handleSaveAutoModels"
          >
            <Icons v-if="savingAutoModels" name="refresh" size="14" class="animate-spin mr-1" />
            <Icons v-else name="check" size="14" class="mr-1" />
            保存优先级配置
          </UiButton>
        </div>
      </div>

      <!-- 当前实时生效的解析顺序预览 -->
      <div v-if="activeModelsOrder && activeModelsOrder.length > 0" class="mb-5 p-3.5 bg-slate-50/80 rounded-xl border border-slate-200/70">
        <div class="text-xs font-semibold text-slate-700 mb-2 flex items-center gap-1.5">
          <Icons name="check" size="14" class="text-emerald-500" />
          当前网关实时动态首选执行链路 (首选 -> 次选 -> 备用):
        </div>
        <div class="flex flex-wrap items-center gap-2">
          <div
            v-for="(target, idx) in activeModelsOrder"
            :key="target"
            class="flex items-center gap-1 text-xs px-2.5 py-1 bg-white border rounded-lg shadow-2xs font-mono"
            :class="idx === 0 ? 'border-emerald-300 text-emerald-800 font-bold bg-emerald-50/50' : 'border-slate-200 text-slate-700'"
          >
            <span class="text-[10px] text-slate-400 font-sans">#{{ idx + 1 }}</span>
            <span>{{ target }}</span>
            <Icons v-if="idx < activeModelsOrder.length - 1" name="chevron-right" size="12" class="text-slate-400 ml-1" />
          </div>
        </div>
      </div>

      <!-- 用户自定义模型顺序编辑列表 -->
      <div class="space-y-2">
        <div class="flex items-center justify-between text-xs font-medium text-slate-600 px-1">
          <span>自定义优先配置序列表 (上移下移调整优先级)</span>
          <span>共 {{ localAutoModels.length }} 个配置项</span>
        </div>

        <div v-if="localAutoModels.length === 0" class="text-center py-6 border border-dashed rounded-xl text-slate-400 text-xs">
          暂无自定义配置，使用系统默认顺序 (gemini-3.8-flash -> deepseek-v4-flash)
        </div>

        <div
          v-for="(model, index) in localAutoModels"
          :key="model"
          class="flex items-center justify-between p-3 bg-white/70 border border-slate-200/80 rounded-xl hover:bg-white transition-all shadow-2xs"
        >
          <div class="flex items-center gap-3">
            <span class="w-6 h-6 rounded-full bg-slate-100 flex items-center justify-center text-xs font-bold text-slate-600">
              {{ index + 1 }}
            </span>
            <span class="font-mono text-xs font-medium text-slate-900">{{ model }}</span>
            <UiBadge v-if="index === 0" variant="default" class="text-[10px] scale-90">
              最高优
            </UiBadge>
          </div>

          <div class="flex items-center gap-1">
            <UiButton
              variant="ghost"
              size="sm"
              class="h-7 w-7 p-0"
              :disabled="index === 0 || !adminWriteEnabled"
              title="上移"
              @click="moveUp(index)"
            >
              <span class="text-xs font-bold">↑</span>
            </UiButton>
            <UiButton
              variant="ghost"
              size="sm"
              class="h-7 w-7 p-0"
              :disabled="index === localAutoModels.length - 1 || !adminWriteEnabled"
              title="下移"
              @click="moveDown(index)"
            >
              <span class="text-xs font-bold">↓</span>
            </UiButton>
            <UiButton
              variant="ghost"
              size="sm"
              class="h-7 w-7 p-0 text-red-500 hover:text-red-700 hover:bg-red-50"
              :disabled="!adminWriteEnabled"
              title="移除"
              @click="removeModel(index)"
            >
              <Icons name="trash" size="14" />
            </UiButton>
          </div>
        </div>

        <!-- 添加新模型 -->
        <div v-if="adminWriteEnabled" class="flex gap-2 pt-2">
          <input
            v-model="newModelInput"
            type="text"
            placeholder="输入主力模型名称 (如 gemini-3.8-flash 或 claude-3-5-sonnet)"
            class="flex-1 text-xs px-3 py-2 border border-slate-200 rounded-lg focus:outline-none focus:ring-1 focus:ring-indigo-500 bg-white"
            @keyup.enter="addModel"
          />
          <UiButton variant="secondary" size="sm" @click="addModel">
            <Icons name="plus" size="14" class="mr-1" />
            添加候选
          </UiButton>
        </div>
      </div>
    </div>
  </div>
</template>
