<script setup lang="ts">
import { ref } from 'vue';
import type { ModelView, CreateModelPayload, UpdateModelPayload } from '../../types/admin';
import { adminApi } from '../../lib/adminApi';
import Icons from '../ui/Icons.vue';
import UiButton from '../ui/UiButton.vue';
import UiBadge from '../ui/UiBadge.vue';
import UiTooltip from '../ui/UiTooltip.vue';
import UiCollapsible from '../ui/UiCollapsible.vue';
import ThinkingEffortSelect from './ThinkingEffortSelect.vue';
import UpstreamModelPicker from './UpstreamModelPicker.vue';
import { formatTierLabel } from '../../utils/format';

const props = defineProps<{
  providerName: string;
  models: ModelView[];
  adminWriteEnabled: boolean;
  defaultExpanded?: boolean;
  /** 批量添加执行器（由视图层注入，内部复用带版本控制的 saveModel 循环）。 */
  onBatchCreate?: (
    provider: string,
    ids: string[],
    onProgress: (done: number, total: number) => void,
  ) => Promise<{ added: number; skipped: number; failed: number }>;
}>();

const emit = defineEmits<{
  (e: 'create', payload: CreateModelPayload): Promise<void>;
  (e: 'update', name: string, payload: UpdateModelPayload): Promise<void>;
  (e: 'delete', name: string): Promise<void>;
  (e: 'notice', message: string): void;
}>();

const isExpanded = ref(props.defaultExpanded ?? false);

const MODEL_TIERS = [
  { value: 'Smart', label: '主力 (Standard)' },
  { value: 'Large', label: '旗舰 (Flagship)' },
  { value: 'Fast', label: '轻量 (Light)' },
] as const;

const CONTEXT_PRESETS = ['256k', '512k', '1m'] as const;

const PROTOCOL_OPTIONS = [
  { value: 'chat', label: 'OpenAI Chat' },
  { value: 'messages', label: 'Anthropic Messages' },
  { value: 'responses', label: 'OpenAI Responses' },
] as const;

const MODALITIES = [
  { id: 'text', icon: 'file-text' as const, label: '文本' },
  { id: 'image', icon: 'image' as const, label: '图片' },
  { id: 'video', icon: 'video' as const, label: '视频' },
  { id: 'audio', icon: 'mic' as const, label: '音频' },
] as const;

const isAdding = ref(false);
const editingModelName = ref<string | null>(null);
const submitting = ref(false);
const formError = ref<string | null>(null);
const showAdvanced = ref(false);
const isCustomContext = ref(false);

const form = ref({
  name: '',
  display_name: '',
  tier: 'Smart',
  context_window: '256k',
  thinking_default: 'Off',
  input_types: ['text', 'image'] as string[],
  output_types: ['text'] as string[],
  protocol: '',
  base_url: '',
  temperature: '',
  top_p: '',
  input_price: '',
  cached_price: '',
  output_price: '',
});

const pickerOpen = ref(false);
const pickerModels = ref<{ id: string }[]>([]);
const pickerLoading = ref(false);
const batchSubmitting = ref(false);
const batchProgress = ref<string | null>(null);

function parseOptionalNumber(raw: string): number | undefined {
  const t = raw.trim();
  if (!t) return undefined;
  const v = Number(t);
  return Number.isFinite(v) ? v : NaN;
}

function normalizeProtocol(proto?: string | null): string {
  if (!proto) return '';
  const lower = proto.toLowerCase();
  if (lower === 'anthropic' || lower === 'claude' || lower === 'messages') return 'messages';
  if (lower === 'chat' || lower === 'openai') return 'chat';
  if (lower === 'responses') return 'responses';
  return proto;
}

function selectProtocol(p: string) {
  if (form.value.protocol === p) {
    form.value.protocol = '';
  } else {
    form.value.protocol = p;
  }
}

function getModalityIcon(mod: string): 'file-text' | 'image' | 'video' | 'mic' | 'sparkles' {
  switch (mod) {
    case 'text': return 'file-text';
    case 'image': return 'image';
    case 'video': return 'video';
    case 'audio': return 'mic';
    default: return 'sparkles';
  }
}

function getModalityName(mod: string): string {
  switch (mod.toLowerCase()) {
    case 'text': return '文本';
    case 'image': return '图像';
    case 'video': return '视频';
    case 'audio': return '音频';
    default: return mod;
  }
}

function openAddInline() {
  editingModelName.value = null;
  form.value = {
    name: '',
    display_name: '',
    tier: 'Smart',
    context_window: '256k',
    thinking_default: 'Off',
    input_types: ['text', 'image'],
    output_types: ['text'],
    protocol: '',
    base_url: '',
    temperature: '',
    top_p: '',
    input_price: '',
    cached_price: '',
    output_price: '',
  };
  isCustomContext.value = false;
  formError.value = null;
  showAdvanced.value = false;
  isExpanded.value = true;
  isAdding.value = true;
}

function openEditInline(model: ModelView) {
  isAdding.value = false;
  isExpanded.value = true;
  editingModelName.value = model.name;
  const cw = model.context_window || '256k';
  const isPreset = (CONTEXT_PRESETS as readonly string[]).includes(cw.toLowerCase());
  isCustomContext.value = !isPreset;

  form.value = {
    name: model.name,
    display_name: model.display_name || '',
    tier: model.tier || 'Smart',
    context_window: cw,
    thinking_default: model.thinking_default || 'Off',
    input_types: model.input_types && model.input_types.length > 0 ? [...model.input_types] : ['text'],
    output_types: model.output_types && model.output_types.length > 0 ? [...model.output_types] : ['text'],
    protocol: normalizeProtocol(model.protocol),
    base_url: model.base_url || '',
    temperature: model.temperature != null ? String(model.temperature) : '',
    top_p: model.top_p != null ? String(model.top_p) : '',
    input_price: model.input_price != null ? String(model.input_price) : '',
    cached_price: model.cached_price != null ? String(model.cached_price) : '',
    output_price: model.output_price != null ? String(model.output_price) : '',
  };
  formError.value = null;
  showAdvanced.value = Boolean(
    model.protocol ||
      model.base_url ||
      model.display_name ||
      model.temperature != null ||
      model.top_p != null ||
      model.input_price != null ||
      model.cached_price != null ||
      model.output_price != null,
  );
}

/** 添加入口：优先拉取该提供商上游模型名单弹窗多选；无接口则 toast 回退手输。 */
async function handleAddClick() {
  if (!props.adminWriteEnabled || isAdding.value) return;
  pickerLoading.value = true;
  try {
    const view = await adminApi.getUpstreamModels(props.providerName).send();
    const ids = (view.models ?? []).map((m) => m.id).filter(Boolean);
    if (ids.length > 0) {
      pickerModels.value = ids.map((id) => ({ id }));
      isExpanded.value = true;
      pickerOpen.value = true;
      return;
    }
    emit('notice', '该提供商未返回模型名单，请手动添加');
    openAddInline();
  } catch {
    emit('notice', '该提供商未提供模型列表接口，请手动添加');
    openAddInline();
  } finally {
    pickerLoading.value = false;
  }
}

async function handlePickerConfirm(ids: string[]) {
  if (!props.onBatchCreate || batchSubmitting.value) return;
  batchSubmitting.value = true;
  batchProgress.value = `添加中 0/${ids.length}`;
  try {
    const summary = await props.onBatchCreate(props.providerName, ids, (done, total) => {
      batchProgress.value = `添加中 ${done}/${total}`;
    });
    const parts = [`已添加 ${summary.added} 个`];
    if (summary.skipped > 0) parts.push(`跳过 ${summary.skipped} 个（已存在）`);
    if (summary.failed > 0) parts.push(`失败 ${summary.failed} 个`);
    emit('notice', parts.join('，'));
  } catch {
    emit('notice', '批量添加失败，请重试');
  } finally {
    batchSubmitting.value = false;
    batchProgress.value = null;
    pickerOpen.value = false;
  }
}

function cancelForm() {
  isAdding.value = false;
  editingModelName.value = null;
  formError.value = null;
}

function setContextPreset(preset: string) {
  isCustomContext.value = false;
  form.value.context_window = preset;
}

function selectCustomContext() {
  isCustomContext.value = true;
}

function toggleInputModality(mod: string) {
  const idx = form.value.input_types.indexOf(mod);
  if (idx >= 0) {
    if (form.value.input_types.length > 1) {
      form.value.input_types.splice(idx, 1);
    }
  } else {
    form.value.input_types.push(mod);
  }
}

function toggleOutputModality(mod: string) {
  const idx = form.value.output_types.indexOf(mod);
  if (idx >= 0) {
    if (form.value.output_types.length > 1) {
      form.value.output_types.splice(idx, 1);
    }
  } else {
    form.value.output_types.push(mod);
  }
}

async function handleSubmit() {
  if (!props.adminWriteEnabled) return;
  const name = form.value.name.trim();
  if (!name) {
    formError.value = '请输入模型名称';
    return;
  }

  submitting.value = true;
  formError.value = null;
  try {
    const temperature = parseOptionalNumber(form.value.temperature);
    const topP = parseOptionalNumber(form.value.top_p);
    const inputPrice = parseOptionalNumber(form.value.input_price);
    const cachedPrice = parseOptionalNumber(form.value.cached_price);
    const outputPrice = parseOptionalNumber(form.value.output_price);
    if (
      temperature !== undefined && !Number.isFinite(temperature) ||
      topP !== undefined && !Number.isFinite(topP) ||
      inputPrice !== undefined && !Number.isFinite(inputPrice) ||
      cachedPrice !== undefined && !Number.isFinite(cachedPrice) ||
      outputPrice !== undefined && !Number.isFinite(outputPrice)
    ) {
      formError.value = '高级参数中的数字格式不正确，请输入合法数值或留空';
      submitting.value = false;
      return;
    }
    const payloadData = {
      provider: props.providerName,
      tier: form.value.tier,
      context_window: form.value.context_window,
      thinking_default: form.value.thinking_default,
      thinking_max: form.value.thinking_default === 'Off' ? 'Off' : 'High',
      input_types: form.value.input_types,
      output_types: form.value.output_types,
      protocol: form.value.protocol ? form.value.protocol.trim() : '',
      base_url: form.value.base_url ? form.value.base_url.trim() : '',
      display_name: form.value.display_name ? form.value.display_name.trim() : '',
      ...(temperature !== undefined ? { temperature } : {}),
      ...(topP !== undefined ? { top_p: topP } : {}),
      ...(inputPrice !== undefined ? { input_price: inputPrice } : {}),
      ...(cachedPrice !== undefined ? { cached_price: cachedPrice } : {}),
      ...(outputPrice !== undefined ? { output_price: outputPrice } : {}),
    };

    if (editingModelName.value) {
      await emit('update', editingModelName.value, payloadData);
    } else {
      await emit('create', {
        name,
        ...payloadData,
      });
    }
    cancelForm();
  } catch (err: unknown) {
    formError.value = err instanceof Error ? err.message : String(err);
  } finally {
    submitting.value = false;
  }
}

async function handleDelete(name: string) {
  if (!props.adminWriteEnabled) return;
  if (!confirm(`确定删除模型 "${name}" 吗？该操作不会中断当前在途请求。`)) {
    return;
  }
  try {
    await emit('delete', name);
  } catch (err: unknown) {
    alert(`删除失败: ${err instanceof Error ? err.message : String(err)}`);
  }
}

function getTierBadgeVariant(tier?: string) {
  switch (tier?.toLowerCase()) {
    case 'fast': return 'success';
    case 'smart': return 'default';
    case 'large': return 'purple';
    default: return 'secondary';
  }
}
</script>

<template>
  <div class="space-y-2">
    <!-- 标题与快捷添加按钮 (支持独立折叠) -->
    <div
      class="flex items-center justify-between pb-1 cursor-pointer select-none"
      @click="isExpanded = !isExpanded"
    >
      <div class="flex items-center gap-1.5 text-xs font-semibold text-slate-700">
        <Icons name="sparkles" size="14" class="text-indigo-500" />
        模型 ({{ models.length }})
        <UiTooltip content="该服务商对外暴露的可路由模型字典及其上下文与思考强度参数">
          <Icons name="info" size="12" class="text-slate-400 cursor-pointer" />
        </UiTooltip>
      </div>

      <div class="flex items-center gap-1" @click.stop>
        <UiButton
          variant="ghost"
          size="sm"
          :disabled="!adminWriteEnabled || isAdding || pickerLoading"
          data-testid="add-model-btn"
          class="text-indigo-600 hover:text-indigo-700 hover:bg-indigo-50/60 font-medium px-2.5 py-1 text-xs"
          @click="handleAddClick"
        >
          <Icons name="plus" size="13" />
          {{ pickerLoading ? '获取中…' : '模型' }}
        </UiButton>
        <UiButton
          variant="ghost"
          size="icon"
          :aria-label="isExpanded ? '收起模型列表' : '展开模型列表'"
          class="text-slate-400 hover:text-slate-600"
          data-testid="toggle-models-btn"
          @click="isExpanded = !isExpanded"
        >
          <Icons :name="isExpanded ? 'chevron-down' : 'chevron-right'" size="14" />
        </UiButton>
      </div>
    </div>

    <!-- 可独立折叠的内容容器 (默认折叠) -->
    <UiCollapsible :open="isExpanded">
      <div class="pt-2 space-y-2">
        <!-- 行内平滑展开新建模型表单 -->
        <UiCollapsible :open="isAdding">
          <div class="p-4 bg-slate-50/90 rounded-xl mb-3 text-xs space-y-3">
            <div class="flex items-center justify-between">
              <span class="font-semibold text-slate-800 text-sm">新建模型配置</span>
          <button
            type="button"
            class="text-slate-400 hover:text-slate-600 cursor-pointer"
            @click="cancelForm"
          >
            <Icons name="cross" size="14" />
          </button>
        </div>

        <div v-if="formError" class="p-2.5 bg-rose-50 text-rose-600 rounded-lg text-xs font-medium">
          {{ formError }}
        </div>

        <form class="space-y-3" @submit.prevent="handleSubmit">
          <!-- 模型 ID 输入 -->
          <div>
            <label class="block text-slate-600 font-medium mb-1 text-xs">模型 ID *</label>
            <input
              v-model="form.name"
              type="text"
              placeholder="例如: gpt-4o，必须与上游模型标识一致"
              required
              autocomplete="off"
              class="w-full bg-white border border-slate-200/80 rounded-lg px-3 py-2 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500"
              data-testid="model-name-input"
            />
          </div>

          <!-- 模型显示名称 (选填) -->
          <div>
            <label class="block text-slate-600 font-medium mb-1 text-xs">显示名称 (选填)</label>
            <input
              v-model="form.display_name"
              type="text"
              placeholder="控制台展示用，留空则显示模型 ID"
              autocomplete="off"
              class="w-full bg-white border border-slate-200/80 rounded-lg px-3 py-2 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500"
              data-testid="model-display-name-input"
            />
          </div>

          <!-- 模型分级 (Tier) 按钮选项组 (暖黄色底色 200 色阶) -->
          <div>
            <label class="block text-slate-600 font-medium mb-1.5 text-xs">模型分级 (Tier)</label>
            <div
              class="grid grid-cols-3 gap-1.5 p-1 bg-amber-200/90 rounded-lg select-none border border-amber-300/50"
              data-testid="model-tier-buttons"
            >
              <button
                v-for="t in MODEL_TIERS"
                :key="t.value"
                type="button"
                :data-testid="`tier-btn-${t.value.toLowerCase()}`"
                class="py-1.5 px-2 rounded-md text-xs font-medium transition-all cursor-pointer text-center"
                :class="[
                  form.tier === t.value
                    ? 'bg-white text-amber-950 shadow-xs font-semibold'
                    : 'text-amber-900/80 hover:text-amber-950 hover:bg-amber-300/60'
                ]"
                @click="form.tier = t.value"
              >
                {{ t.label }}
              </button>
            </div>
          </div>

          <!-- 上下文窗口 (Context Window) 按钮选项组 (仅保留 256K, 512K, 1M) -->
          <div>
            <label class="block text-slate-600 font-medium mb-1.5 text-xs">上下文窗口 (Context Window)</label>
            <div
              class="flex flex-wrap items-center gap-1.5 p-1 bg-slate-200/60 rounded-lg select-none mb-1.5"
              data-testid="context-window-buttons"
            >
              <button
                v-for="p in CONTEXT_PRESETS"
                :key="p"
                type="button"
                :data-testid="`context-btn-${p}`"
                class="flex-1 py-1.5 px-2 rounded-md text-xs font-medium transition-all cursor-pointer text-center"
                :class="[
                  !isCustomContext && form.context_window?.toLowerCase() === p.toLowerCase()
                    ? 'bg-white text-indigo-700 shadow-2xs font-semibold'
                    : 'text-slate-600 hover:text-slate-900 hover:bg-white/40'
                ]"
                @click="setContextPreset(p)"
              >
                {{ p.toUpperCase() }}
              </button>
              <button
                type="button"
                data-testid="context-btn-custom"
                class="py-1.5 px-3 rounded-md text-xs font-medium transition-all cursor-pointer text-center"
                :class="[
                  isCustomContext
                    ? 'bg-white text-indigo-700 shadow-2xs font-semibold'
                    : 'text-slate-600 hover:text-slate-900 hover:bg-white/40'
                ]"
                @click="selectCustomContext"
              >
                自定义
              </button>
            </div>

            <UiCollapsible :open="isCustomContext">
              <input
                v-model="form.context_window"
                type="text"
                placeholder="输入自定义上下文容量，例如: 256k"
                class="w-full bg-white border border-slate-200/80 rounded-lg px-3 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500 mt-1"
                data-testid="model-context-window-input"
              />
            </UiCollapsible>
          </div>

          <!-- 思考强度按钮组 (无最大上限) -->
          <ThinkingEffortSelect
            v-model:default-effort="form.thinking_default"
          />

          <!-- 支持模态类型拆解：输入模态与输出模态独立选择器 -->
          <div class="space-y-2.5 p-2.5 bg-white/70 rounded-lg border border-slate-200/70" data-testid="model-modalities-section">
            <div>
              <label class="block text-slate-600 font-medium mb-1.5 text-xs flex items-center gap-1.5">
                <span>输入模态 (Input)</span>
                <UiTooltip content="该模型支持接收的输入模态能力 (点击纯图标切换)">
                  <Icons name="info" size="12" class="text-slate-400 cursor-pointer" />
                </UiTooltip>
              </label>
              <div class="flex items-center gap-2" data-testid="model-input-modalities">
                <UiTooltip
                  v-for="m in MODALITIES"
                  :key="`in-${m.id}`"
                  :content="`输入支持: ${m.label} (点击切换)`"
                >
                  <button
                    type="button"
                    :data-testid="`input-modality-btn-${m.id}`"
                    class="h-8 w-8 rounded-lg flex items-center justify-center transition-all cursor-pointer"
                    :class="[
                      form.input_types.includes(m.id)
                        ? 'bg-indigo-50 text-indigo-600 ring-1 ring-indigo-300 shadow-2xs'
                        : 'bg-slate-100 text-slate-400 hover:text-slate-600 hover:bg-slate-200/60'
                    ]"
                    @click="toggleInputModality(m.id)"
                  >
                    <Icons :name="m.icon" size="15" />
                  </button>
                </UiTooltip>
              </div>
            </div>

            <div>
              <label class="block text-slate-600 font-medium mb-1.5 text-xs flex items-center gap-1.5">
                <span>输出模态 (Output)</span>
                <UiTooltip content="该模型能够生成的输出模态能力 (点击纯图标切换)">
                  <Icons name="info" size="12" class="text-slate-400 cursor-pointer" />
                </UiTooltip>
              </label>
              <div class="flex items-center gap-2" data-testid="model-output-modalities">
                <UiTooltip
                  v-for="m in MODALITIES"
                  :key="`out-${m.id}`"
                  :content="`输出支持: ${m.label} (点击切换)`"
                >
                  <button
                    type="button"
                    :data-testid="`output-modality-btn-${m.id}`"
                    class="h-8 w-8 rounded-lg flex items-center justify-center transition-all cursor-pointer"
                    :class="[
                      form.output_types.includes(m.id)
                        ? 'bg-emerald-50 text-emerald-600 ring-1 ring-emerald-300 shadow-2xs'
                        : 'bg-slate-100 text-slate-400 hover:text-slate-600 hover:bg-slate-200/60'
                    ]"
                    @click="toggleOutputModality(m.id)"
                  >
                    <Icons :name="m.icon" size="15" />
                  </button>
                </UiTooltip>
              </div>
            </div>
          </div>

          <!-- 下级折叠层级：标题直接是“高级” -->
          <div class="pt-1">
            <button
              type="button"
              class="text-xs font-semibold text-slate-600 hover:text-indigo-600 inline-flex items-center gap-1 cursor-pointer py-1 select-none"
              data-testid="toggle-advanced-btn"
              @click="showAdvanced = !showAdvanced"
            >
              <Icons :name="showAdvanced ? 'chevron-down' : 'chevron-right'" size="12" />
              高级
            </button>

            <UiCollapsible :open="showAdvanced">
              <div class="p-3 bg-slate-100/70 rounded-lg space-y-3 mt-1.5">
                <!-- 3 种底层协议选项 -->
                <div>
                  <label class="block text-slate-600 font-medium mb-1.5 text-xs">底层协议覆盖 (选填)</label>
                  <div class="flex flex-wrap gap-1.5" data-testid="model-protocol-options">
                    <button
                      v-for="opt in PROTOCOL_OPTIONS"
                      :key="opt.value"
                      type="button"
                      :data-testid="`model-proto-${opt.value}`"
                      class="px-2.5 py-1 rounded-md text-xs font-medium transition-all cursor-pointer border"
                      :class="[
                        form.protocol === opt.value
                          ? 'bg-indigo-600 text-white border-indigo-600 shadow-2xs font-semibold'
                          : 'bg-white text-slate-600 border-slate-200 hover:border-slate-300'
                      ]"
                      @click="selectProtocol(opt.value)"
                    >
                      {{ opt.label }}
                    </button>
                  </div>
                  <p class="text-3xs text-slate-400 mt-1">未选中时继承服务商默认协议；点击已选协议可取消选择。</p>
                </div>

                <!-- 专属 base_url 输入框 -->
                <div>
                  <label class="block text-slate-600 font-medium mb-1 text-xs">模型专属 Base URL (选填)</label>
                  <input
                    v-model="form.base_url"
                    type="text"
                    placeholder="例如: https://api.openai.com/v1 或留空继承服务商配置"
                    class="w-full bg-white border border-slate-200/80 rounded-lg px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500"
                    data-testid="model-base-url-input"
                  />
                </div>

                <!-- 默认采样参数 (请求未传时生效) -->
                <div class="grid grid-cols-2 gap-2">
                  <div>
                    <label class="block text-slate-600 font-medium mb-1 text-xs">默认 Temperature (选填)</label>
                    <input
                      v-model="form.temperature"
                      type="text"
                      inputmode="decimal"
                      placeholder="留空继承，如 0.7"
                      class="w-full bg-white border border-slate-200/80 rounded-lg px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500"
                      data-testid="model-temperature-input"
                    />
                  </div>
                  <div>
                    <label class="block text-slate-600 font-medium mb-1 text-xs">默认 Top P (选填)</label>
                    <input
                      v-model="form.top_p"
                      type="text"
                      inputmode="decimal"
                      placeholder="留空继承，如 0.9"
                      class="w-full bg-white border border-slate-200/80 rounded-lg px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500"
                      data-testid="model-top-p-input"
                    />
                  </div>
                </div>
                <p class="text-3xs text-slate-400 -mt-2">请求里显式传了 temperature/top_p 时，以请求值为准。</p>

                <!-- 模型专属价格 ($/1M tokens，留空继承服务商) -->
                <div>
                  <label class="block text-slate-600 font-medium mb-1.5 text-xs">模型专属价格 (选填，留空继承服务商)</label>
                  <div class="grid grid-cols-3 gap-2">
                    <div>
                      <label class="block text-slate-500 mb-1 text-3xs">输入</label>
                      <input
                        v-model="form.input_price"
                        type="text"
                        inputmode="decimal"
                        placeholder="如 0.50"
                        class="w-full bg-white border border-slate-200/80 rounded-lg px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500"
                        data-testid="model-input-price-input"
                      />
                    </div>
                    <div>
                      <label class="block text-slate-500 mb-1 text-3xs">缓存命中</label>
                      <input
                        v-model="form.cached_price"
                        type="text"
                        inputmode="decimal"
                        placeholder="如 0.25"
                        class="w-full bg-white border border-slate-200/80 rounded-lg px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500"
                        data-testid="model-cached-price-input"
                      />
                    </div>
                    <div>
                      <label class="block text-slate-500 mb-1 text-3xs">输出</label>
                      <input
                        v-model="form.output_price"
                        type="text"
                        inputmode="decimal"
                        placeholder="如 1.00"
                        class="w-full bg-white border border-slate-200/80 rounded-lg px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500"
                        data-testid="model-output-price-input"
                      />
                    </div>
                  </div>
                </div>
              </div>
            </UiCollapsible>
          </div>

          <div class="flex items-center justify-end gap-2 pt-2 border-t border-slate-200/60">
            <UiButton variant="ghost" size="sm" @click="cancelForm">
              取消
            </UiButton>
            <UiButton
              type="submit"
              size="sm"
              :disabled="submitting || !adminWriteEnabled"
              data-testid="submit-model-btn"
            >
              {{ submitting ? '保存中...' : '保存模型' }}
            </UiButton>
          </div>
        </form>
      </div>
    </UiCollapsible>

    <!-- 模型条目列表 (彻底去除第三层边框与背景，平滑轻盈) -->
    <div v-if="models.length === 0" class="py-3 text-center text-xs text-slate-400 bg-transparent border-none rounded-lg">
      暂未注册模型，点击上方「+ 模型」快速挂载
    </div>

    <div v-else class="space-y-1">
      <div
        v-for="m in models"
        :key="m.name"
        class="bg-transparent hover:bg-white/35 rounded-lg border-none transition-colors text-xs overflow-hidden"
        data-testid="model-row"
      >
        <!-- 一等常显行 -->
        <div class="flex items-center justify-between px-3.5 py-2.5">
          <div class="flex items-center gap-2.5 min-w-0">
            <span class="font-semibold text-slate-800 text-sm truncate" :title="m.display_name ? `${m.display_name} (${m.name})` : m.name">{{ m.display_name || m.name }}</span>
            <span v-if="m.display_name" class="text-slate-400 text-xs font-mono truncate" :title="m.name">{{ m.name }}</span>
            <UiBadge :variant="getTierBadgeVariant(m.tier)">
              {{ formatTierLabel(m.tier) }}
            </UiBadge>
            <span class="text-slate-400 text-xs font-mono">{{ m.context_window }}</span>

            <!-- 多模态简明纯图标常显指示 (输入与输出分开) -->
            <div class="hidden sm:flex items-center gap-1.5 text-slate-400" data-testid="model-row-modalities">
              <span class="text-3xs text-slate-400 font-mono">入:</span>
              <span v-for="t in (m.input_types && m.input_types.length > 0 ? m.input_types : ['text'])" :key="`row-in-${t}`" class="inline-flex items-center">
                <UiTooltip :content="`输入支持: ${getModalityName(t)}`">
                  <Icons :name="getModalityIcon(t)" size="12" class="text-slate-500" />
                </UiTooltip>
              </span>
              <span class="text-slate-300 text-3xs">/</span>
              <span class="text-3xs text-slate-400 font-mono">出:</span>
              <span v-for="t in (m.output_types && m.output_types.length > 0 ? m.output_types : ['text'])" :key="`row-out-${t}`" class="inline-flex items-center">
                <UiTooltip :content="`输出支持: ${getModalityName(t)}`">
                  <Icons :name="getModalityIcon(t)" size="12" class="text-emerald-600" />
                </UiTooltip>
              </span>
            </div>

            <!-- 底层协议标签 (若有定制) -->
            <UiBadge
              v-if="m.protocol"
              variant="secondary"
              class="hidden md:inline-flex items-center text-3xs font-mono font-normal"
              :title="m.base_url ? `协议: ${m.protocol} · Base URL: ${m.base_url}` : `协议: ${m.protocol}`"
            >
              {{ m.protocol }}
            </UiBadge>

            <!-- 采样/价格定制标记 -->
            <UiTooltip
              v-if="m.temperature != null || m.top_p != null"
              :content="`默认采样: ${m.temperature != null ? `temperature=${m.temperature}` : ''}${m.temperature != null && m.top_p != null ? ' · ' : ''}${m.top_p != null ? `top_p=${m.top_p}` : ''} (请求显式传参时以请求为准)`"
            >
              <UiBadge variant="secondary" class="hidden md:inline-flex items-center text-3xs font-mono font-normal cursor-help">
                T{{ m.temperature != null ? `=${m.temperature}` : '' }}{{ m.top_p != null ? ` P=${m.top_p}` : '' }}
              </UiBadge>
            </UiTooltip>
            <UiTooltip
              v-if="m.input_price != null || m.cached_price != null || m.output_price != null"
              :content="`专属价格($/1M): 入${m.input_price ?? '继承'} / 缓${m.cached_price ?? '继承'} / 出${m.output_price ?? '继承'}`"
            >
              <UiBadge variant="secondary" class="hidden md:inline-flex items-center text-3xs font-mono font-normal cursor-help">
                ￥定制
              </UiBadge>
            </UiTooltip>

            <!-- 思考强度简明标记 (当非 Off 时展示微标) -->
            <UiTooltip
              v-if="m.thinking_default && m.thinking_default !== 'Off'"
              :content="`思考强度预设: ${m.thinking_default}`"
            >
              <UiBadge variant="purple" class="inline-flex items-center gap-1 cursor-help">
                <Icons name="brain" size="11" />
                {{ m.thinking_default }}
              </UiBadge>
            </UiTooltip>
          </div>

          <div class="flex items-center gap-1.5 shrink-0">
            <!-- 编辑纯图标按钮 -->
            <UiTooltip content="编辑模型参数">
              <UiButton
                variant="ghost"
                size="icon"
                :aria-label="`编辑模型 ${m.name}`"
                :disabled="!adminWriteEnabled"
                data-testid="edit-model-btn"
                class="text-slate-500 hover:text-indigo-600 hover:bg-indigo-50"
                @click="openEditInline(m)"
              >
                <Icons name="edit" size="14" />
              </UiButton>
            </UiTooltip>

            <!-- 删除纯图标按钮 -->
            <UiTooltip content="移除此模型">
              <UiButton
                variant="ghost"
                size="icon"
                :aria-label="`删除模型 ${m.name}`"
                :disabled="!adminWriteEnabled"
                data-testid="delete-model-btn"
                class="text-slate-400 hover:text-rose-600 hover:bg-rose-50"
                @click="handleDelete(m.name)"
              >
                <Icons name="trash" size="14" />
              </UiButton>
            </UiTooltip>
          </div>
        </div>

        <!-- 当前模型的平滑内联编辑区 -->
        <UiCollapsible :open="editingModelName === m.name">
          <div class="p-4 bg-white border-t border-slate-100 text-xs space-y-3">
            <div class="flex items-center justify-between gap-3 min-w-0">
              <span class="font-semibold text-slate-800 text-sm truncate" :title="m.name">编辑模型: {{ m.name }}</span>
              <button
                type="button"
                class="text-slate-400 hover:text-slate-600 cursor-pointer shrink-0 p-1"
                aria-label="关闭编辑"
                @click="cancelForm"
              >
                <Icons name="cross" size="14" />
              </button>
            </div>

            <div v-if="formError" class="p-2.5 bg-rose-50 text-rose-600 rounded-lg text-xs font-medium">
              {{ formError }}
            </div>

            <form class="space-y-3" @submit.prevent="handleSubmit">
              <!-- 显示名称 (选填，仅控制台展示) -->
              <div>
                <label class="block text-slate-600 font-medium mb-1 text-xs">显示名称 (选填)</label>
                <input
                  v-model="form.display_name"
                  type="text"
                  placeholder="留空则显示模型 ID"
                  autocomplete="off"
                  class="w-full bg-white border border-slate-200/80 rounded-lg px-3 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500"
                  data-testid="model-display-name-input"
                />
              </div>

              <!-- 模型分级 (Tier) 按钮选项组 (暖黄色底色 200 色阶) -->
              <div>
                <label class="block text-slate-600 font-medium mb-1.5 text-xs">模型分级 (Tier)</label>
                <div
                  class="grid grid-cols-3 gap-1.5 p-1 bg-amber-200/90 rounded-lg select-none border border-amber-300/50"
                  data-testid="model-tier-buttons"
                >
                  <button
                    v-for="t in MODEL_TIERS"
                    :key="t.value"
                    type="button"
                    :data-testid="`tier-btn-${t.value.toLowerCase()}`"
                    class="py-1.5 px-2 rounded-md text-xs font-medium transition-all cursor-pointer text-center"
                    :class="[
                      form.tier === t.value
                        ? 'bg-white text-amber-950 shadow-xs font-semibold'
                        : 'text-amber-900/80 hover:text-amber-950 hover:bg-amber-300/60'
                    ]"
                    @click="form.tier = t.value"
                  >
                    {{ t.label }}
                  </button>
                </div>
              </div>

              <!-- 上下文窗口 (Context Window) 按钮选项组 (仅保留 256K, 512K, 1M) -->
              <div>
                <label class="block text-slate-600 font-medium mb-1.5 text-xs">上下文窗口 (Context Window)</label>
                <div
                  class="flex flex-wrap items-center gap-1.5 p-1 bg-slate-100 rounded-lg select-none mb-1.5"
                  data-testid="context-window-buttons"
                >
                  <button
                    v-for="p in CONTEXT_PRESETS"
                    :key="p"
                    type="button"
                    :data-testid="`context-btn-${p}`"
                    class="flex-1 py-1.5 px-2 rounded-md text-xs font-medium transition-all cursor-pointer text-center"
                    :class="[
                      !isCustomContext && form.context_window?.toLowerCase() === p.toLowerCase()
                        ? 'bg-white text-indigo-700 shadow-2xs font-semibold'
                        : 'text-slate-600 hover:text-slate-900 hover:bg-white/40'
                    ]"
                    @click="setContextPreset(p)"
                  >
                    {{ p.toUpperCase() }}
                  </button>
                  <button
                    type="button"
                    data-testid="context-btn-custom"
                    class="py-1.5 px-3 rounded-md text-xs font-medium transition-all cursor-pointer text-center"
                    :class="[
                      isCustomContext
                        ? 'bg-white text-indigo-700 shadow-2xs font-semibold'
                        : 'text-slate-600 hover:text-slate-900 hover:bg-white/40'
                    ]"
                    @click="selectCustomContext"
                  >
                    自定义
                  </button>
                </div>

                <UiCollapsible :open="isCustomContext">
                  <input
                    v-model="form.context_window"
                    type="text"
                    placeholder="输入自定义上下文容量，例如: 256k"
                    class="w-full bg-white border border-slate-200/80 rounded-lg px-3 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500 mt-1"
                    data-testid="model-context-window-input"
                  />
                </UiCollapsible>
              </div>

              <!-- 思考强度按钮组 (无最大上限) -->
              <ThinkingEffortSelect
                v-model:default-effort="form.thinking_default"
                :disabled="!adminWriteEnabled"
              />

              <!-- 支持模态类型拆解：输入模态与输出模态独立选择器 -->
              <div class="space-y-2.5 p-2.5 bg-slate-50/80 rounded-lg border border-slate-200/60" data-testid="model-modalities-section">
                <div>
                  <label class="block text-slate-600 font-medium mb-1.5 text-xs flex items-center gap-1.5">
                    <span>输入模态 (Input)</span>
                    <UiTooltip content="该模型支持接收的输入模态能力 (点击纯图标切换)">
                      <Icons name="info" size="12" class="text-slate-400 cursor-pointer" />
                    </UiTooltip>
                  </label>
                  <div class="flex items-center gap-2" data-testid="model-input-modalities">
                    <UiTooltip
                      v-for="m in MODALITIES"
                      :key="`in-${m.id}`"
                      :content="`输入支持: ${m.label} (点击切换)`"
                    >
                      <button
                        type="button"
                        :data-testid="`input-modality-btn-${m.id}`"
                        class="h-8 w-8 rounded-lg flex items-center justify-center transition-all cursor-pointer"
                        :class="[
                          form.input_types.includes(m.id)
                            ? 'bg-indigo-50 text-indigo-600 ring-1 ring-indigo-300 shadow-2xs'
                            : 'bg-slate-100 text-slate-400 hover:text-slate-600 hover:bg-slate-200/60'
                        ]"
                        @click="toggleInputModality(m.id)"
                      >
                        <Icons :name="m.icon" size="15" />
                      </button>
                    </UiTooltip>
                  </div>
                </div>

                <div>
                  <label class="block text-slate-600 font-medium mb-1.5 text-xs flex items-center gap-1.5">
                    <span>输出模态 (Output)</span>
                    <UiTooltip content="该模型能够生成的输出模态能力 (点击纯图标切换)">
                      <Icons name="info" size="12" class="text-slate-400 cursor-pointer" />
                    </UiTooltip>
                  </label>
                  <div class="flex items-center gap-2" data-testid="model-output-modalities">
                    <UiTooltip
                      v-for="m in MODALITIES"
                      :key="`out-${m.id}`"
                      :content="`输出支持: ${m.label} (点击切换)`"
                    >
                      <button
                        type="button"
                        :data-testid="`output-modality-btn-${m.id}`"
                        class="h-8 w-8 rounded-lg flex items-center justify-center transition-all cursor-pointer"
                        :class="[
                          form.output_types.includes(m.id)
                            ? 'bg-emerald-50 text-emerald-600 ring-1 ring-emerald-300 shadow-2xs'
                            : 'bg-slate-100 text-slate-400 hover:text-slate-600 hover:bg-slate-200/60'
                        ]"
                        @click="toggleOutputModality(m.id)"
                      >
                        <Icons :name="m.icon" size="15" />
                      </button>
                    </UiTooltip>
                  </div>
                </div>
              </div>

              <!-- 下级折叠层级：标题直接是“高级” -->
              <div class="pt-1">
                <button
                  type="button"
                  class="text-xs font-semibold text-slate-600 hover:text-indigo-600 inline-flex items-center gap-1 cursor-pointer py-1 select-none"
                  data-testid="toggle-advanced-btn"
                  @click="showAdvanced = !showAdvanced"
                >
                  <Icons :name="showAdvanced ? 'chevron-down' : 'chevron-right'" size="12" />
                  高级
                </button>

                <UiCollapsible :open="showAdvanced">
                  <div class="p-3 bg-slate-100/70 rounded-lg space-y-3 mt-1.5">
                    <!-- 3 种底层协议选项 -->
                    <div>
                      <label class="block text-slate-600 font-medium mb-1.5 text-xs">底层协议覆盖 (选填)</label>
                      <div class="flex flex-wrap gap-1.5" data-testid="model-protocol-options">
                        <button
                          v-for="opt in PROTOCOL_OPTIONS"
                          :key="opt.value"
                          type="button"
                          :data-testid="`model-proto-${opt.value}`"
                          class="px-2.5 py-1 rounded-md text-xs font-medium transition-all cursor-pointer border"
                          :class="[
                            form.protocol === opt.value
                              ? 'bg-indigo-600 text-white border-indigo-600 shadow-2xs font-semibold'
                              : 'bg-white text-slate-600 border-slate-200 hover:border-slate-300'
                          ]"
                          @click="selectProtocol(opt.value)"
                        >
                          {{ opt.label }}
                        </button>
                      </div>
                      <p class="text-3xs text-slate-400 mt-1">未选中时继承服务商默认协议；点击已选协议可取消选择。</p>
                    </div>

                    <!-- 专属 base_url 输入框 -->
                    <div>
                      <label class="block text-slate-600 font-medium mb-1 text-xs">模型专属 Base URL (选填)</label>
                      <input
                        v-model="form.base_url"
                        type="text"
                        placeholder="例如: https://api.openai.com/v1 或留空继承服务商配置"
                        class="w-full bg-white border border-slate-200/80 rounded-lg px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500"
                        data-testid="model-base-url-input"
                      />
                    </div>

                    <!-- 默认采样参数 (请求未传时生效) -->
                    <div class="grid grid-cols-2 gap-2">
                      <div>
                        <label class="block text-slate-600 font-medium mb-1 text-xs">默认 Temperature (选填)</label>
                        <input
                          v-model="form.temperature"
                          type="text"
                          inputmode="decimal"
                          placeholder="留空继承，如 0.7"
                          class="w-full bg-white border border-slate-200/80 rounded-lg px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500"
                          data-testid="model-temperature-input"
                        />
                      </div>
                      <div>
                        <label class="block text-slate-600 font-medium mb-1 text-xs">默认 Top P (选填)</label>
                        <input
                          v-model="form.top_p"
                          type="text"
                          inputmode="decimal"
                          placeholder="留空继承，如 0.9"
                          class="w-full bg-white border border-slate-200/80 rounded-lg px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500"
                          data-testid="model-top-p-input"
                        />
                      </div>
                    </div>
                    <p class="text-3xs text-slate-400 -mt-2">请求里显式传了 temperature/top_p 时，以请求值为准。</p>

                    <!-- 模型专属价格 ($/1M tokens，留空继承服务商) -->
                    <div>
                      <label class="block text-slate-600 font-medium mb-1.5 text-xs">模型专属价格 (选填，留空继承服务商)</label>
                      <div class="grid grid-cols-3 gap-2">
                        <div>
                          <label class="block text-slate-500 mb-1 text-3xs">输入</label>
                          <input
                            v-model="form.input_price"
                            type="text"
                            inputmode="decimal"
                            placeholder="如 0.50"
                            class="w-full bg-white border border-slate-200/80 rounded-lg px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500"
                            data-testid="model-input-price-input"
                          />
                        </div>
                        <div>
                          <label class="block text-slate-500 mb-1 text-3xs">缓存命中</label>
                          <input
                            v-model="form.cached_price"
                            type="text"
                            inputmode="decimal"
                            placeholder="如 0.25"
                            class="w-full bg-white border border-slate-200/80 rounded-lg px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500"
                            data-testid="model-cached-price-input"
                          />
                        </div>
                        <div>
                          <label class="block text-slate-500 mb-1 text-3xs">输出</label>
                          <input
                            v-model="form.output_price"
                            type="text"
                            inputmode="decimal"
                            placeholder="如 1.00"
                            class="w-full bg-white border border-slate-200/80 rounded-lg px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500"
                            data-testid="model-output-price-input"
                          />
                        </div>
                      </div>
                    </div>
                  </div>
                </UiCollapsible>
              </div>

              <div class="flex items-center justify-end gap-2 pt-2 border-t border-slate-100">
                <UiButton variant="ghost" size="sm" @click="cancelForm">
                  取消
                </UiButton>
                <UiButton
                  type="submit"
                  size="sm"
                  :disabled="submitting || !adminWriteEnabled"
                  data-testid="update-model-btn"
                >
                  {{ submitting ? '保存中...' : '更新' }}
                </UiButton>
              </div>
            </form>
          </div>
        </UiCollapsible>
      </div>
    </div>
    </div>
    </UiCollapsible>

    <!-- 上游模型多选弹窗 -->
    <UpstreamModelPicker
      v-if="pickerOpen"
      :provider-name="providerName"
      :models="pickerModels"
      :existing-names="models.map((m) => m.name)"
      :submitting="batchSubmitting"
      :progress="batchProgress"
      @confirm="handlePickerConfirm"
      @close="pickerOpen = false"
    />
  </div>
</template>
