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

/** 目录里出现过的 provider（升序），作为链项第一段的下拉选项。 */
const providerOptions = computed(() => {
  const set = new Set<string>();
  for (const m of props.allModels || []) {
    if (m.provider) set.add(m.provider);
  }
  return [...set].sort();
});

function modelsForProvider(provider: string): string[] {
  return (props.allModels || [])
    .filter((m) => m.provider === provider)
    .map((m) => m.name);
}

/**
 * 链项 `provider/model` 的分解：取**第一段**为 provider，其余归 model
 * （与后端 `split_once('/')` 一致，因此 `magpie/trae-cn/xxx` 这类
 * 自带斜杠的物理名不会被截断）。
 */
function splitEntry(entry: string): { provider: string; model: string } {
  const i = entry.indexOf('/');
  if (i < 0) return { provider: '', model: entry };
  return { provider: entry.slice(0, i), model: entry.slice(i + 1) };
}

function composeEntry(provider: string, model: string): string {
  if (!provider) return model;
  if (!model) return provider;
  return `${provider}/${model}`;
}

/** 链项自身的 provider 选项：目录里的，加上当前值（未登记时不静默丢弃）。 */
function entryProviders(entry: string): string[] {
  const { provider } = splitEntry(entry);
  const opts = providerOptions.value;
  return provider && !opts.includes(provider) ? [provider, ...opts] : opts;
}

/** 链项自身的模型选项：该 provider 下的，加上当前值（未登记时不静默丢弃）。 */
function entryModels(entry: string): string[] {
  const { provider, model } = splitEntry(entry);
  const opts = modelsForProvider(provider);
  return model && !opts.includes(model) ? [model, ...opts] : opts;
}

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
  const firstProvider = providerOptions.value[0] || '';
  const firstModel = modelsForProvider(firstProvider)[0] || '';
  entry.chain = [...entry.chain, composeEntry(firstProvider, firstModel)];
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

function setChainProvider(modelIndex: number, entryIndex: number, provider: string) {
  const list = [...localModels.value];
  const entry = list[modelIndex];
  const chain = [...entry.chain];
  const current = splitEntry(chain[entryIndex] || '');
  const candidates = modelsForProvider(provider);
  // 保留仍属该 provider 的当前模型；否则落到该 provider 的第一个模型。
  const nextModel = candidates.includes(current.model) ? current.model : candidates[0] || '';
  chain[entryIndex] = composeEntry(provider, nextModel);
  entry.chain = chain;
  localModels.value = list;
}

function setChainModel(modelIndex: number, entryIndex: number, model: string) {
  const list = [...localModels.value];
  const entry = list[modelIndex];
  const chain = [...entry.chain];
  const { provider } = splitEntry(chain[entryIndex] || '');
  chain[entryIndex] = composeEntry(provider, model);
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
            <select
              :value="splitEntry(entry).provider"
              :data-testid="`aggregate-chain-provider-${entryIndex}`"
              :disabled="!adminWriteEnabled"
              aria-label="链项 provider"
              class="w-32 shrink-0 bg-white border border-slate-200/80 rounded-lg px-2 py-1.5 text-xs font-mono text-slate-800 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-500"
              @change="
                setChainProvider(
                  modelIndex,
                  entryIndex,
                  ($event.target as HTMLSelectElement).value,
                )
              "
            >
              <option value="">（裸模型名）</option>
              <option v-for="p in entryProviders(entry)" :key="p" :value="p">{{ p }}</option>
            </select>
            <select
              :value="splitEntry(entry).model"
              :data-testid="`aggregate-chain-entry-${entryIndex}`"
              :disabled="!adminWriteEnabled"
              aria-label="链项模型"
              class="flex-1 min-w-0 bg-white border border-slate-200/80 rounded-lg px-2 py-1.5 text-xs font-mono text-slate-800 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-500"
              @change="
                setChainModel(modelIndex, entryIndex, ($event.target as HTMLSelectElement).value)
              "
            >
              <option value="">（选择模型）</option>
              <option v-for="m in entryModels(entry)" :key="m" :value="m">{{ m }}</option>
            </select>
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
