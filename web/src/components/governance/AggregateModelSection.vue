<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import Icons from '../ui/Icons.vue';
import UiBadge from '../ui/UiBadge.vue';
import UiButton from '../ui/UiButton.vue';
import type { AggregateModelView, ModelView } from '../../types/admin';

/**
 * 聚合模型（虚拟名 + 有序优先级链）编辑区。
 *
 * 语义契约：`.agents/notes/implemented/architecture/2026-11-23-aggregate-model-priority-chain.md`
 * —— 链序即执行序（不参与价格/延迟重排）；某项失败后冷却 `cooldown_secs`，
 * 到期自动回到原位，从而"以免费主力为主、异源兜底、到期回主"。
 */
const props = defineProps<{
  adminWriteEnabled: boolean;
  aggregateModels?: AggregateModelView[];
  /** 全量模型目录（`useAdminConfig.models`），用于链项建议列表。 */
  allModels?: ModelView[];
}>();

const emit = defineEmits<{
  (e: 'updateAggregateModels', models: AggregateModelView[]): Promise<void>;
}>();

const DEFAULT_COOLDOWN_SECS = 90;

const localModels = ref<AggregateModelView[]>(clone(props.aggregateModels || []));
const saving = ref(false);
const formError = ref<string | null>(null);

function clone(list: AggregateModelView[]): AggregateModelView[] {
  return list.map((m) => ({
    name: m.name,
    chain: [...m.chain],
    cooldown_secs: m.cooldown_secs,
  }));
}

watch(
  () => props.aggregateModels,
  (val) => {
    if (val) localModels.value = clone(val);
  },
  { deep: true },
);

/** 目录里出现过的 `provider/model`，作为链项输入的建议值（仍允许手输未登记的物理名）。 */
const chainSuggestions = computed(() =>
  (props.allModels || [])
    .filter((m) => Boolean(m.provider))
    .map((m) => `${m.provider}/${m.name}`),
);

const hasDuplicateName = computed(() => {
  const seen = new Set<string>();
  for (const m of localModels.value) {
    const key = m.name.trim().toLowerCase();
    if (!key) continue;
    if (seen.has(key)) return true;
    seen.add(key);
  }
  return false;
});

const canSave = computed(
  () =>
    props.adminWriteEnabled &&
    !saving.value &&
    !hasDuplicateName.value &&
    localModels.value.every((m) => m.name.trim().length > 0),
);

function addModel() {
  if (!props.adminWriteEnabled) return;
  const used = new Set(localModels.value.map((m) => m.name));
  let name = 'deepseek-free';
  let seq = 2;
  while (used.has(name)) {
    name = `deepseek-free-${seq}`;
    seq += 1;
  }
  localModels.value = [
    ...localModels.value,
    { name, chain: [], cooldown_secs: DEFAULT_COOLDOWN_SECS },
  ];
}

function removeModel(index: number) {
  if (!props.adminWriteEnabled) return;
  const list = [...localModels.value];
  list.splice(index, 1);
  localModels.value = list;
}

function addChainEntry(modelIndex: number) {
  if (!props.adminWriteEnabled) return;
  const list = [...localModels.value];
  const entry = list[modelIndex];
  const suggestion = chainSuggestions.value[0] || '';
  entry.chain = [...entry.chain, suggestion];
  localModels.value = list;
}

function removeChainEntry(modelIndex: number, entryIndex: number) {
  if (!props.adminWriteEnabled) return;
  const list = [...localModels.value];
  const entry = list[modelIndex];
  const chain = [...entry.chain];
  chain.splice(entryIndex, 1);
  entry.chain = chain;
  localModels.value = list;
}

function moveChainEntry(modelIndex: number, entryIndex: number, delta: -1 | 1) {
  if (!props.adminWriteEnabled) return;
  const list = [...localModels.value];
  const entry = list[modelIndex];
  const target = entryIndex + delta;
  if (target < 0 || target >= entry.chain.length) return;
  const chain = [...entry.chain];
  const [moved] = chain.splice(entryIndex, 1);
  chain.splice(target, 0, moved);
  entry.chain = chain;
  localModels.value = list;
}

function setChainEntry(modelIndex: number, entryIndex: number, value: string) {
  const list = [...localModels.value];
  const entry = list[modelIndex];
  const chain = [...entry.chain];
  chain[entryIndex] = value;
  entry.chain = chain;
  localModels.value = list;
}

function setCooldown(modelIndex: number, raw: string) {
  const list = [...localModels.value];
  const entry = list[modelIndex];
  const parsed = Number.parseInt(raw, 10);
  entry.cooldown_secs = Number.isFinite(parsed) && parsed > 0 ? parsed : 1;
  localModels.value = list;
}

async function handleSave() {
  if (!canSave.value) return;
  const invalid = localModels.value.find((m) => m.chain.length === 0);
  if (invalid) {
    formError.value = `聚合模型 '${invalid.name}' 至少要有一个链项`;
    return;
  }
  // 链项两侧空白会让路由匹配落空（后端按逐字 `provider/model` 解析）。
  const payload = localModels.value.map((m) => ({
    ...m,
    name: m.name.trim(),
    chain: m.chain.map((c) => c.trim()).filter((c) => c.length > 0),
  }));
  formError.value = null;
  saving.value = true;
  try {
    await emit('updateAggregateModels', payload);
  } catch (err: unknown) {
    formError.value = err instanceof Error ? err.message : String(err);
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <div class="swiss-card p-6">
    <div class="flex items-start justify-between mb-4 gap-3">
      <div>
        <h2 class="text-base font-bold text-slate-900 flex items-center gap-2">
          <Icons name="repeat" size="18" class="text-indigo-600" />
          聚合模型（优先级链）
        </h2>
        <p class="text-xs text-slate-500 mt-1">
          一个客户端可见的虚拟模型名 + 一条有序 <code class="px-1 py-0.5 bg-slate-100 rounded text-slate-800 font-mono">provider/model</code> 链。
          链序即执行顺序：主力失败进入冷却（默认 90s），冷却期内由下一项顶上，到期自动回到原位。
        </p>
      </div>
      <div class="flex items-center gap-2 shrink-0">
        <UiButton
          variant="ghost"
          size="sm"
          data-testid="aggregate-model-add"
          :disabled="!adminWriteEnabled"
          @click="addModel"
        >
          <Icons name="plus" size="14" class="mr-1" />
          新建聚合模型
        </UiButton>
        <UiButton
          variant="default"
          size="sm"
          data-testid="aggregate-model-save"
          :disabled="!canSave"
          @click="handleSave"
        >
          <Icons v-if="saving" name="refresh" size="14" class="animate-spin mr-1" />
          <Icons v-else name="check" size="14" class="mr-1" />
          保存聚合配置
        </UiButton>
      </div>
    </div>

    <p
      v-if="hasDuplicateName"
      class="mb-3 text-xs text-red-600"
      data-testid="aggregate-model-duplicate-error"
    >
      聚合模型名重复：每个虚拟名必须唯一。
    </p>
    <p v-if="formError" class="mb-3 text-xs text-red-600" data-testid="aggregate-model-error">
      {{ formError }}
    </p>

    <div
      v-if="localModels.length === 0"
      class="text-center py-6 border border-dashed rounded-xl text-slate-400 text-xs"
      data-testid="aggregate-model-empty"
    >
      暂无聚合模型。新建一个即可把「以免费主力为先、异源兜底、到期回主」固化为一条链。
    </div>

    <div class="space-y-3">
      <div
        v-for="(model, modelIndex) in localModels"
        :key="`aggregate-${modelIndex}`"
        class="p-3 bg-slate-100/60 border border-slate-200/70 rounded-xl space-y-3"
        data-testid="aggregate-model-card"
      >
        <div class="flex flex-wrap items-center gap-2">
          <input
            :value="model.name"
            data-testid="aggregate-model-name"
            placeholder="虚拟模型名，如 deepseek-free"
            :disabled="!adminWriteEnabled"
            class="flex-1 min-w-[180px] bg-white border border-slate-200/80 rounded-lg px-3 py-1.5 text-xs font-mono text-slate-800 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-500"
            @input="model.name = ($event.target as HTMLInputElement).value"
          />
          <label class="flex items-center gap-1 text-[11px] text-slate-600">
            冷却
            <input
              :value="model.cooldown_secs"
              data-testid="aggregate-model-cooldown"
              type="number"
              min="1"
              :disabled="!adminWriteEnabled"
              class="w-20 bg-white border border-slate-200/80 rounded-lg px-2 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-500"
              @input="setCooldown(modelIndex, ($event.target as HTMLInputElement).value)"
            />
            秒
          </label>
          <span
            class="text-[11px] text-slate-400"
            data-testid="aggregate-model-chain-is-authorization"
            title="链即授权：链里写了谁就按顺序用谁（含付费项），不再有单独的付费开关"
          >
            链即授权
          </span>
          <UiButton
            variant="ghost"
            size="sm"
            class="h-7 px-2 text-red-500 hover:text-red-700 hover:bg-red-50"
            :data-testid="`aggregate-model-remove-${modelIndex}`"
            :disabled="!adminWriteEnabled"
            @click="removeModel(modelIndex)"
          >
            删除
          </UiButton>
        </div>

        <div class="space-y-1.5">
          <div class="flex items-center justify-between text-[11px] font-medium text-slate-600 px-0.5">
            <span>优先级链（从上到下尝试）</span>
            <span>{{ model.chain.length }} 项</span>
          </div>

          <div
            v-for="(entry, entryIndex) in model.chain"
            :key="`entry-${modelIndex}-${entryIndex}`"
            class="flex items-center gap-1.5"
            data-testid="aggregate-chain-row"
          >
            <span
              class="w-5 h-5 rounded-full bg-white border border-slate-200 flex items-center justify-center text-[10px] font-bold text-slate-600 shrink-0"
            >
              {{ entryIndex + 1 }}
            </span>
            <input
              :value="entry"
              :list="`aggregate-chain-suggestions-${modelIndex}`"
              data-testid="aggregate-chain-entry"
              placeholder="provider/model，如 sense/deepseek-flash"
              :disabled="!adminWriteEnabled"
              class="flex-1 bg-white border border-slate-200/80 rounded-lg px-3 py-1.5 text-xs font-mono text-slate-800 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-500"
              @input="setChainEntry(modelIndex, entryIndex, ($event.target as HTMLInputElement).value)"
            />
            <UiBadge v-if="entryIndex === 0" variant="default" class="text-[10px] scale-90">
              主力
            </UiBadge>
            <UiButton
              variant="ghost"
              size="sm"
              class="h-7 w-7 p-0"
              :data-testid="`aggregate-chain-up-${entryIndex}`"
              :disabled="entryIndex === 0 || !adminWriteEnabled"
              title="上移"
              :aria-label="`上移第 ${entryIndex + 1} 项`"
              @click="moveChainEntry(modelIndex, entryIndex, -1)"
            >
              <span class="text-xs font-bold">↑</span>
            </UiButton>
            <UiButton
              variant="ghost"
              size="sm"
              class="h-7 w-7 p-0"
              :data-testid="`aggregate-chain-down-${entryIndex}`"
              :disabled="entryIndex === model.chain.length - 1 || !adminWriteEnabled"
              title="下移"
              :aria-label="`下移第 ${entryIndex + 1} 项`"
              @click="moveChainEntry(modelIndex, entryIndex, 1)"
            >
              <span class="text-xs font-bold">↓</span>
            </UiButton>
            <UiButton
              variant="ghost"
              size="sm"
              class="h-7 w-7 p-0 text-red-500 hover:text-red-700 hover:bg-red-50"
              :data-testid="`aggregate-chain-remove-${entryIndex}`"
              :disabled="!adminWriteEnabled"
              title="删除该项"
              :aria-label="`删除第 ${entryIndex + 1} 项`"
              @click="removeChainEntry(modelIndex, entryIndex)"
            >
              ×
            </UiButton>
          </div>

          <datalist :id="`aggregate-chain-suggestions-${modelIndex}`">
            <option v-for="hint in chainSuggestions" :key="hint" :value="hint" />
          </datalist>

          <UiButton
            variant="ghost"
            size="sm"
            :data-testid="`aggregate-chain-add-${modelIndex}`"
            :disabled="!adminWriteEnabled"
            @click="addChainEntry(modelIndex)"
          >
            <Icons name="plus" size="12" class="mr-1" />
            添加链项
          </UiButton>
        </div>
      </div>
    </div>

    <p class="mt-3 text-[11px] text-slate-400">
      提示：链项多写 `provider/model` 可消解各家命名差异（sense/deepseek-flash →
      magpie/deepseek-v4.1-flash → workbuddy/deepseek-v4.1-flash）；写成裸模型名则表示「任意提供该名的 provider」。
    </p>
  </div>
</template>
