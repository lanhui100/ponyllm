<script setup lang="ts">
import { ref } from 'vue';
import type { ModelView, CreateModelPayload, UpdateModelPayload, PricingMode, PricingPeriod } from '../../types/admin';
import { adminApi } from '../../lib/adminApi';
import Icons from '../ui/Icons.vue';
import UiButton from '../ui/UiButton.vue';
import UiBadge from '../ui/UiBadge.vue';
import UiTooltip from '../ui/UiTooltip.vue';
import UiCollapsible from '../ui/UiCollapsible.vue';
import ThinkingEffortSelect from './ThinkingEffortSelect.vue';
import UpstreamModelPicker from './UpstreamModelPicker.vue';
import HourRangePicker from './HourRangePicker.vue';
import { formatTierLabel, formatContextWindow } from '../../utils/format';
import { toast } from '../../composables/useToast';

const props = defineProps<{
  providerName: string;
  models: ModelView[];
  adminWriteEnabled: boolean;
  defaultExpanded?: boolean;
  onDeleteModel?: (name: string, provider?: string) => Promise<void>;
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

interface EditablePricingPeriod {
  name?: string;
  start_time: string;
  end_time: string;
  input_price: string;
  cached_price: string;
  output_price: string;
  include_weekends: boolean;
}

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
  pricing_mode: 'uniform' as PricingMode,
  input_price: '',
  cached_price: '',
  output_price: '',
  pricing_periods: [] as EditablePricingPeriod[],
});

function addPricingPeriod() {
  form.value.pricing_periods.push({
    name: '',
    start_time: '08:00',
    end_time: '24:00',
    input_price: '',
    cached_price: '',
    output_price: '',
    include_weekends: false,
  });
}

function removePricingPeriod(index: number) {
  form.value.pricing_periods.splice(index, 1);
}

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
    pricing_mode: 'uniform',
    input_price: '',
    cached_price: '',
    output_price: '',
    pricing_periods: [],
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

  const periods: EditablePricingPeriod[] = (model.pricing_periods || []).map((p) => ({
    name: p.name || '',
    start_time: p.start_time,
    end_time: p.end_time,
    input_price: String(p.input_price),
    cached_price: String(p.cached_price),
    output_price: String(p.output_price),
    include_weekends: Boolean(p.include_weekends),
  }));

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
    pricing_mode: (model.pricing_mode as PricingMode) || 'uniform',
    input_price: model.input_price != null ? String(model.input_price) : '',
    cached_price: model.cached_price != null ? String(model.cached_price) : '',
    output_price: model.output_price != null ? String(model.output_price) : '',
    pricing_periods: periods,
  };
  formError.value = null;
  showAdvanced.value = Boolean(
    model.protocol ||
      model.base_url ||
      model.display_name ||
      model.temperature != null ||
      model.top_p != null ||
      model.pricing_mode === 'peak_valley' ||
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
    let validPeriods: PricingPeriod[] = [];
    for (const p of form.value.pricing_periods) {
      const inP = parseOptionalNumber(p.input_price) ?? 0;
      const caP = parseOptionalNumber(p.cached_price) ?? 0;
      const outP = parseOptionalNumber(p.output_price) ?? 0;
      if (!Number.isFinite(inP) || !Number.isFinite(caP) || !Number.isFinite(outP) || inP < 0 || caP < 0 || outP < 0) {
        formError.value = `峰价时段的价格必须为大于等于 0 的有效数值`;
        submitting.value = false;
        return;
      }
      validPeriods.push({
        name: '',
        start_time: p.start_time.trim() || '08:00',
        end_time: p.end_time.trim() || '24:00',
        input_price: inP,
        cached_price: caP,
        output_price: outP,
        include_weekends: Boolean(p.include_weekends),
      });
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
      pricing_mode: (validPeriods.length > 0 ? 'peak_valley' : 'uniform') as PricingMode,
      pricing_periods: validPeriods,
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
  const confirmed = await toast.confirm({
    title: '删除模型',
    message: `确定删除模型 "${name}" 吗？该操作不会中断当前在途请求。`,
    confirmText: '确认删除',
    cancelText: '取消',
    variant: 'destructive',
  });
  if (!confirmed) {
    return;
  }
  try {
    if (props.onDeleteModel) {
      await props.onDeleteModel(name, props.providerName);
    } else {
      emit('delete', name);
    }
    toast.success(`模型 "${name}" 已成功删除`);
  } catch (err: unknown) {
    toast.error(`删除失败: ${err instanceof Error ? err.message : String(err)}`);
  }
}

function getTierBadgeVariant(tier?: string) {
  switch (tier?.toLowerCase()) {
    case 'fast': return 'success';
    case 'smart': return 'default';
    case 'large': return 'secondary';
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
        <Icons name="sparkles" size="14" class="text-slate-500" />
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
          class="text-slate-800 hover:text-slate-950 hover:bg-slate-100/70 font-medium px-2.5 py-1 text-xs"
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
          <div class="p-5 bg-white/70 backdrop-blur-md border border-white/60 shadow-xs rounded-xl mb-3 text-xs space-y-4">
            <div class="flex items-center justify-between pb-2 border-b border-slate-200/50">
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

            <form class="space-y-6" @submit.prevent="handleSubmit">
              <!-- 模型 ID 输入 (同一行2列) -->
              <div class="grid grid-cols-1 sm:grid-cols-12 gap-2 sm:gap-4 items-center">
                <label class="sm:col-span-3 text-slate-700 font-medium text-xs">模型 ID *</label>
                <div class="sm:col-span-9">
                  <input
                    v-model="form.name"
                    type="text"
                    placeholder="例如: gpt-4o，必须与上游模型标识一致"
                    required
                    autocomplete="off"
                    class="w-full bg-white border border-slate-200/80 rounded-lg px-3 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-500"
                    data-testid="model-name-input"
                  />
                </div>
              </div>

              <!-- 模型显示名称 (选填，同一行2列) -->
              <div class="grid grid-cols-1 sm:grid-cols-12 gap-2 sm:gap-4 items-center">
                <label class="sm:col-span-3 text-slate-700 font-medium text-xs">显示名称 (选填)</label>
                <div class="sm:col-span-9">
                  <input
                    v-model="form.display_name"
                    type="text"
                    placeholder="控制台展示用，留空则显示模型 ID"
                    autocomplete="off"
                    class="w-full bg-white border border-slate-200/80 rounded-lg px-3 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-500"
                    data-testid="model-display-name-input"
                  />
                </div>
              </div>

              <!-- 模型分级 (Tier) 按钮选项组 (同一行2列) -->
              <div class="grid grid-cols-1 sm:grid-cols-12 gap-2 sm:gap-4 items-center">
                <label class="sm:col-span-3 text-slate-700 font-medium text-xs">模型分级 (Tier)</label>
                <div class="sm:col-span-9">
                  <div
                    class="grid grid-cols-3 gap-1.5 p-1 bg-slate-100/90 rounded-lg select-none border border-slate-200/70"
                    data-testid="model-tier-buttons"
                  >
                    <button
                      v-for="t in MODEL_TIERS"
                      :key="t.value"
                      type="button"
                      :data-testid="`tier-btn-${t.value.toLowerCase()}`"
                      class="py-1 px-2 rounded-md text-xs font-medium transition-all cursor-pointer text-center"
                      :class="[
                        form.tier === t.value
                          ? 'bg-slate-900 text-white shadow-2xs font-semibold'
                          : 'text-slate-600 hover:text-slate-900 hover:bg-white/50'
                      ]"
                      @click="form.tier = t.value"
                    >
                      {{ t.label }}
                    </button>
                  </div>
                </div>
              </div>

              <!-- 上下文窗口 (Context Window) 按钮选项组 (同一行2列) -->
              <div class="grid grid-cols-1 sm:grid-cols-12 gap-2 sm:gap-4 items-center">
                <label class="sm:col-span-3 text-slate-700 font-medium text-xs">上下文窗口</label>
                <div class="sm:col-span-9">
                  <div
                    class="flex flex-wrap items-center gap-1.5 p-1 bg-slate-100/90 rounded-lg select-none border border-slate-200/70"
                    data-testid="context-window-buttons"
                  >
                    <button
                      v-for="p in CONTEXT_PRESETS"
                      :key="p"
                      type="button"
                      :data-testid="`context-btn-${p}`"
                      class="flex-1 py-1 px-2 rounded-md text-xs font-medium transition-all cursor-pointer text-center"
                      :class="[
                        !isCustomContext && form.context_window?.toLowerCase() === p.toLowerCase()
                          ? 'bg-slate-900 text-white shadow-2xs font-semibold'
                          : 'text-slate-600 hover:text-slate-900 hover:bg-white/50'
                      ]"
                      @click="setContextPreset(p)"
                    >
                      {{ p.toUpperCase() }}
                    </button>
                    <button
                      type="button"
                      data-testid="context-btn-custom"
                      class="py-1 px-3 rounded-md text-xs font-medium transition-all cursor-pointer text-center"
                      :class="[
                        isCustomContext
                          ? 'bg-slate-900 text-white shadow-2xs font-semibold'
                          : 'text-slate-600 hover:text-slate-900 hover:bg-white/50'
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
                      class="w-full bg-white border border-slate-200/80 rounded-lg px-3 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-500 mt-1"
                      data-testid="model-context-window-input"
                    />
                  </UiCollapsible>
                </div>
              </div>

              <!-- 思考强度 (同一行2列) -->
              <div class="grid grid-cols-1 sm:grid-cols-12 gap-2 sm:gap-4 items-center">
                <label class="sm:col-span-3 text-slate-700 font-medium text-xs flex items-center gap-1">
                  <span>思考强度</span>
                  <UiTooltip content="设置模型推理思考深度的预设档位，按需开启深度认知">
                    <Icons name="info" size="12" class="text-slate-400 cursor-pointer" />
                  </UiTooltip>
                </label>
                <div class="sm:col-span-9">
                  <ThinkingEffortSelect
                    v-model:default-effort="form.thinking_default"
                  />
                </div>
              </div>

              <!-- 输入模态与输出模态 (按两列分别排列) -->
              <div class="grid grid-cols-1 sm:grid-cols-12 gap-2 sm:gap-4 items-center" data-testid="model-modalities-section">
                <label class="sm:col-span-3 text-slate-700 font-medium text-xs">模态支持</label>
                <div class="sm:col-span-9 grid grid-cols-1 sm:grid-cols-2 gap-3">
                  <!-- 输入模态列 -->
                  <div class="flex items-center justify-between p-2.5 bg-slate-100/70 rounded-lg border border-slate-200/60" data-testid="model-input-modalities">
                    <span class="text-xs text-slate-700 font-medium select-none">输入:</span>
                    <div class="flex items-center gap-1.5">
                      <UiTooltip
                        v-for="m in MODALITIES"
                        :key="`in-${m.id}`"
                        :content="`输入支持: ${m.label} (点击切换)`"
                      >
                        <button
                          type="button"
                          :data-testid="`input-modality-btn-${m.id}`"
                          class="h-7 w-7 rounded-md flex items-center justify-center transition-all cursor-pointer"
                          :class="[
                            form.input_types.includes(m.id)
                              ? 'bg-slate-900 text-white shadow-2xs font-semibold'
                              : 'bg-white text-slate-400 hover:text-slate-700 hover:bg-slate-50 border border-slate-200/60'
                          ]"
                          @click="toggleInputModality(m.id)"
                        >
                          <Icons :name="m.icon" size="14" />
                        </button>
                      </UiTooltip>
                    </div>
                  </div>

                  <!-- 输出模态列 -->
                  <div class="flex items-center justify-between p-2.5 bg-slate-100/70 rounded-lg border border-slate-200/60" data-testid="model-output-modalities">
                    <span class="text-xs text-slate-700 font-medium select-none">输出:</span>
                    <div class="flex items-center gap-1.5">
                      <UiTooltip
                        v-for="m in MODALITIES"
                        :key="`out-${m.id}`"
                        :content="`输出支持: ${m.label} (点击切换)`"
                      >
                        <button
                          type="button"
                          :data-testid="`output-modality-btn-${m.id}`"
                          class="h-7 w-7 rounded-md flex items-center justify-center transition-all cursor-pointer"
                          :class="[
                            form.output_types.includes(m.id)
                              ? 'bg-slate-900 text-white shadow-2xs font-semibold'
                              : 'bg-white text-slate-400 hover:text-slate-700 hover:bg-slate-50 border border-slate-200/60'
                          ]"
                          @click="toggleOutputModality(m.id)"
                        >
                          <Icons :name="m.icon" size="14" />
                        </button>
                      </UiTooltip>
                    </div>
                  </div>
                </div>
              </div>

              <!-- 下级折叠层级：标题直接是“高级” -->
              <div class="pt-1">
                <button
                  type="button"
                  class="text-xs font-semibold text-slate-600 hover:text-slate-900 inline-flex items-center gap-1 cursor-pointer py-1 select-none"
                  data-testid="toggle-advanced-btn"
                  @click="showAdvanced = !showAdvanced"
                >
                  <Icons :name="showAdvanced ? 'chevron-down' : 'chevron-right'" size="12" />
                  高级
                </button>

                <UiCollapsible :open="showAdvanced">
                  <div class="p-3.5 bg-slate-100/70 backdrop-blur-xs rounded-lg space-y-3.5 mt-1.5 border border-slate-200/60">
                    <!-- 3 种底层协议选项 (同一行2列) -->
                    <div class="grid grid-cols-1 sm:grid-cols-12 gap-2 sm:gap-4 items-center">
                      <label class="sm:col-span-3 text-slate-600 font-medium text-xs">协议覆盖</label>
                      <div class="sm:col-span-9">
                        <div class="flex flex-wrap gap-1.5" data-testid="model-protocol-options">
                          <button
                            v-for="opt in PROTOCOL_OPTIONS"
                            :key="opt.value"
                            type="button"
                            :data-testid="`model-proto-${opt.value}`"
                            class="px-2.5 py-1 rounded-md text-xs font-medium transition-all cursor-pointer border"
                            :class="[
                              form.protocol === opt.value
                                ? 'bg-slate-900 text-white border-slate-900 shadow-2xs font-semibold'
                                : 'bg-white text-slate-600 border-slate-200 hover:border-slate-300'
                            ]"
                            @click="selectProtocol(opt.value)"
                          >
                            {{ opt.label }}
                          </button>
                        </div>
                        <p class="text-3xs text-slate-400 mt-1">未选中时继承服务商默认协议；点击已选协议可取消选择。</p>
                      </div>
                    </div>

                    <!-- 专属 base_url 输入框 (同一行2列) -->
                    <div class="grid grid-cols-1 sm:grid-cols-12 gap-2 sm:gap-4 items-center">
                      <label class="sm:col-span-3 text-slate-600 font-medium text-xs">Base URL</label>
                      <div class="sm:col-span-9">
                        <input
                          v-model="form.base_url"
                          type="text"
                          placeholder="例如: https://api.openai.com/v1 或留空继承服务商配置"
                          class="w-full bg-white border border-slate-200/80 rounded-lg px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-500"
                          data-testid="model-base-url-input"
                        />
                      </div>
                    </div>

                    <!-- 默认采样参数 (请求未传时生效，同一行2列) -->
                    <div class="grid grid-cols-1 sm:grid-cols-12 gap-2 sm:gap-4 items-center">
                      <label class="sm:col-span-3 text-slate-600 font-medium text-xs">默认采样</label>
                      <div class="sm:col-span-9 grid grid-cols-2 gap-2">
                        <div>
                          <input
                            v-model="form.temperature"
                            type="text"
                            inputmode="decimal"
                            placeholder="Temperature 如 0.7"
                            class="w-full bg-white border border-slate-200/80 rounded-lg px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-500"
                            data-testid="model-temperature-input"
                          />
                        </div>
                        <div>
                          <input
                            v-model="form.top_p"
                            type="text"
                            inputmode="decimal"
                            placeholder="Top P 如 0.9"
                            class="w-full bg-white border border-slate-200/80 rounded-lg px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-500"
                            data-testid="model-top-p-input"
                          />
                        </div>
                      </div>
                    </div>

                    <!-- 价格与峰谷模式 (同一行2列：默认价格为谷价，可添加峰价时段) -->
                    <div class="grid grid-cols-1 sm:grid-cols-12 gap-2 sm:gap-4 items-start pt-1">
                      <div class="sm:col-span-3 text-slate-600 font-medium text-xs pt-1.5">
                        <span>价格</span>
                        <p class="text-3xs text-slate-400 font-normal mt-0.5">￥/M tokens</p>
                      </div>
                      <div class="sm:col-span-9 space-y-3">
                        <!-- 默认价格 (谷价) 基础价格卡片 -->
                        <div class="p-3 bg-slate-100/60 rounded-lg border border-slate-200/60">
                          <div class="flex items-center justify-between mb-2">
                            <span class="text-xs font-semibold text-slate-800 flex items-center gap-1">
                              <span>默认价格（谷价）</span>
                              <UiTooltip content="未进入峰价特别时段或非峰价周末时，统一按此单价计费">
                                <Icons name="info" size="12" class="text-slate-400 cursor-pointer" />
                              </UiTooltip>
                            </span>
                            <span class="text-3xs text-slate-400 font-mono">常规/非高峰</span>
                          </div>
                          <div class="grid grid-cols-3 gap-2.5">
                            <div>
                              <label class="block text-slate-500 mb-0.5 text-3xs">输入单价</label>
                              <input
                                v-model="form.input_price"
                                type="text"
                                inputmode="decimal"
                                placeholder="0.00"
                                class="w-full bg-white border border-slate-200/80 rounded px-2.5 py-1 text-xs text-slate-800 font-mono focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-500"
                                data-testid="model-input-price-input"
                              />
                            </div>
                            <div>
                              <label class="block text-slate-500 mb-0.5 text-3xs">缓存单价</label>
                              <input
                                v-model="form.cached_price"
                                type="text"
                                inputmode="decimal"
                                placeholder="0.00"
                                class="w-full bg-white border border-slate-200/80 rounded px-2.5 py-1 text-xs text-slate-800 font-mono focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-500"
                                data-testid="model-cached-price-input"
                              />
                            </div>
                            <div>
                              <label class="block text-slate-500 mb-0.5 text-3xs">输出单价</label>
                              <input
                                v-model="form.output_price"
                                type="text"
                                inputmode="decimal"
                                placeholder="0.00"
                                class="w-full bg-white border border-slate-200/80 rounded px-2.5 py-1 text-xs text-slate-800 font-mono focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-500"
                                data-testid="model-output-price-input"
                              />
                            </div>
                          </div>
                        </div>

                        <!-- 峰价特别时段列表与添加按钮 (无空白行，用户点击添加时增加) -->
                        <div class="space-y-2.5">
                          <div v-if="form.pricing_periods.length > 0" class="flex items-center justify-between text-xs text-slate-700 font-medium">
                            <span class="flex items-center gap-1">
                              <span class="w-1.5 h-1.5 rounded-full bg-amber-500"></span>
                              峰价特别时段
                            </span>
                            <span class="text-3xs text-slate-400">仅在指定时段执行以下单价</span>
                          </div>

                          <div
                            v-for="(period, pIdx) in form.pricing_periods"
                            :key="`period-${pIdx}`"
                            class="p-2.5 bg-white/80 rounded-lg border border-slate-200/70 text-xs space-y-2"
                          >
                            <div class="flex flex-wrap items-center gap-2.5 justify-between">
                              <!-- 0-23整点时段选择器与包含周末勾选 -->
                              <div class="flex items-center gap-2 shrink-0">
                                <span class="text-xs text-amber-700 font-medium bg-amber-50 px-1.5 py-0.5 rounded border border-amber-200/60">峰时</span>
                                <HourRangePicker
                                  v-model:start-time="period.start_time"
                                  v-model:end-time="period.end_time"
                                />
                                <label class="inline-flex items-center gap-1 text-xs text-slate-600 select-none cursor-pointer">
                                  <input
                                    v-model="period.include_weekends"
                                    type="checkbox"
                                    class="rounded border-slate-300 text-slate-900 focus:ring-0 cursor-pointer"
                                  />
                                  <span>包含周末</span>
                                </label>
                              </div>

                              <!-- 紧凑单价输入框 (输入、缓存、输出单价并排一行) -->
                              <div class="flex items-center gap-2.5 grow sm:grow-0 justify-end">
                                <div class="flex items-center gap-1">
                                  <span class="text-3xs text-slate-500 shrink-0 font-medium">输入:</span>
                                  <input
                                    v-model="period.input_price"
                                    type="text"
                                    inputmode="decimal"
                                    placeholder="0.00"
                                    class="w-16 bg-white border border-slate-200 rounded px-1.5 py-1 text-xs text-slate-800 font-mono text-right"
                                  />
                                </div>
                                <div class="flex items-center gap-1">
                                  <span class="text-3xs text-slate-500 shrink-0 font-medium">缓存:</span>
                                  <input
                                    v-model="period.cached_price"
                                    type="text"
                                    inputmode="decimal"
                                    placeholder="0.00"
                                    class="w-16 bg-white border border-slate-200 rounded px-1.5 py-1 text-xs text-slate-800 font-mono text-right"
                                  />
                                </div>
                                <div class="flex items-center gap-1">
                                  <span class="text-3xs text-slate-500 shrink-0 font-medium">输出:</span>
                                  <input
                                    v-model="period.output_price"
                                    type="text"
                                    inputmode="decimal"
                                    placeholder="0.00"
                                    class="w-16 bg-white border border-slate-200 rounded px-1.5 py-1 text-xs text-slate-800 font-mono text-right"
                                  />
                                </div>

                                <button
                                  type="button"
                                  class="text-slate-400 hover:text-rose-600 p-1 cursor-pointer ml-0.5 shrink-0"
                                  title="删除峰价时段"
                                  @click="removePricingPeriod(pIdx)"
                                >
                                  <Icons name="trash" size="13" />
                                </button>
                              </div>
                            </div>
                          </div>

                          <button
                            type="button"
                            class="w-full py-1.5 border border-dashed border-slate-300 hover:border-slate-400 rounded-lg text-xs text-slate-600 hover:text-slate-900 flex items-center justify-center gap-1 cursor-pointer bg-slate-50/50"
                            @click="addPricingPeriod"
                          >
                            <Icons name="plus" size="13" />
                            添加峰价特别时段
                          </button>
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
        class="bg-transparent hover:bg-white/35 rounded-lg border-none transition-colors text-xs"
        data-testid="model-row"
      >
        <!-- 一等常显行 -->
        <div class="flex items-center justify-between px-3.5 py-2.5">
          <div class="flex items-center gap-2.5 min-w-0">
            <span class="font-semibold text-slate-800 text-sm truncate" :title="m.display_name ? `${m.display_name} (${m.name})` : m.name">{{ m.display_name || m.name }}</span>
            <span v-if="m.display_name" class="text-slate-400 text-xs font-mono truncate" :title="m.name">{{ m.name }}</span>
            <!-- 模型分级与思考强度核心徽标组 (位置统一紧邻排列) -->
            <UiBadge :variant="getTierBadgeVariant(m.tier)">
              {{ formatTierLabel(m.tier) }}
            </UiBadge>

            <!-- 思考强度简明标记 (紧随模型分级，当非 Off 时展示微标) -->
            <UiTooltip
              v-if="m.thinking_default && m.thinking_default !== 'Off'"
              :content="`思考强度预设: ${m.thinking_default}`"
            >
              <UiBadge variant="secondary" class="inline-flex items-center gap-1 cursor-help">
                <Icons name="brain" size="12" />
                {{ m.thinking_default }}
              </UiBadge>
            </UiTooltip>

            <!-- 统一规范化上下文容量 (消除大小写混用) -->
            <span class="inline-flex items-center px-2 py-0.5 rounded-md bg-slate-100/90 text-slate-700 text-xs font-mono font-medium border border-slate-200/60 shadow-2xs">
              {{ formatContextWindow(m.context_window) }}
            </span>

            <!-- 多模态简明纯图标常显指示 (输入与输出分开，统一图标尺寸与胶囊高度) -->
            <div class="hidden sm:inline-flex items-center gap-1.5 px-2 py-0.5 bg-slate-100/80 rounded-md border border-slate-200/60 shadow-2xs text-slate-500" data-testid="model-row-modalities">
              <span class="text-3xs text-slate-500 font-medium select-none">入:</span>
              <div class="inline-flex items-center gap-1">
                <span v-for="t in (m.input_types && m.input_types.length > 0 ? m.input_types : ['text'])" :key="`row-in-${t}`" class="inline-flex items-center">
                  <UiTooltip :content="`输入支持: ${getModalityName(t)}`">
                    <Icons :name="getModalityIcon(t)" size="14" class="text-slate-600 hover:text-slate-900 transition-colors" />
                  </UiTooltip>
                </span>
              </div>
              <span class="text-slate-300 text-xs select-none">|</span>
              <span class="text-3xs text-slate-500 font-medium select-none">出:</span>
              <div class="inline-flex items-center gap-1">
                <span v-for="t in (m.output_types && m.output_types.length > 0 ? m.output_types : ['text'])" :key="`row-out-${t}`" class="inline-flex items-center">
                  <UiTooltip :content="`输出支持: ${getModalityName(t)}`">
                    <Icons :name="getModalityIcon(t)" size="14" class="text-slate-600 hover:text-slate-900 transition-colors" />
                  </UiTooltip>
                </span>
              </div>
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
              v-if="m.pricing_mode === 'peak_valley' || m.input_price != null || m.cached_price != null || m.output_price != null"
              :content="m.pricing_mode === 'peak_valley' ? `峰谷价格: 谷价(入${m.input_price ?? '继承'}/缓${m.cached_price ?? '继承'}/出${m.output_price ?? '继承'}) + ${m.pricing_periods?.length || 0}个峰时` : `价格(￥/M): 入${m.input_price ?? '继承'} / 缓${m.cached_price ?? '继承'} / 出${m.output_price ?? '继承'}`"
            >
              <UiBadge variant="secondary" class="hidden md:inline-flex items-center text-3xs font-mono font-normal cursor-help">
                {{ m.pricing_mode === 'peak_valley' ? '峰谷定价' : '￥定制' }}
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
                class="text-slate-500 hover:text-slate-900 hover:bg-slate-100"
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
          <div class="p-5 bg-white/70 backdrop-blur-md border border-white/60 shadow-xs rounded-xl text-xs space-y-4 my-2">
            <div class="flex items-center justify-between gap-3 min-w-0 pb-2 border-b border-slate-200/50">
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

            <form class="space-y-6" @submit.prevent="handleSubmit">
              <!-- 显示名称 (选填，仅控制台展示，同一行2列) -->
              <div class="grid grid-cols-1 sm:grid-cols-12 gap-2 sm:gap-4 items-center">
                <label class="sm:col-span-3 text-slate-700 font-medium text-xs">显示名称 (选填)</label>
                <div class="sm:col-span-9">
                  <input
                    v-model="form.display_name"
                    type="text"
                    placeholder="留空则显示模型 ID"
                    autocomplete="off"
                    class="w-full bg-white border border-slate-200/80 rounded-lg px-3 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-500"
                    data-testid="model-display-name-input"
                  />
                </div>
              </div>

              <!-- 模型分级 (Tier) 按钮选项组 (同一行2列) -->
              <div class="grid grid-cols-1 sm:grid-cols-12 gap-2 sm:gap-4 items-center">
                <label class="sm:col-span-3 text-slate-700 font-medium text-xs">模型分级 (Tier)</label>
                <div class="sm:col-span-9">
                  <div
                    class="grid grid-cols-3 gap-1.5 p-1 bg-slate-100/90 rounded-lg select-none border border-slate-200/70"
                    data-testid="model-tier-buttons"
                  >
                    <button
                      v-for="t in MODEL_TIERS"
                      :key="t.value"
                      type="button"
                      :data-testid="`tier-btn-${t.value.toLowerCase()}`"
                      class="py-1 px-2 rounded-md text-xs font-medium transition-all cursor-pointer text-center"
                      :class="[
                        form.tier === t.value
                          ? 'bg-slate-900 text-white shadow-2xs font-semibold'
                          : 'text-slate-600 hover:text-slate-900 hover:bg-white/50'
                      ]"
                      @click="form.tier = t.value"
                    >
                      {{ t.label }}
                    </button>
                  </div>
                </div>
              </div>

              <!-- 上下文窗口 (Context Window) 按钮选项组 (同一行2列) -->
              <div class="grid grid-cols-1 sm:grid-cols-12 gap-2 sm:gap-4 items-center">
                <label class="sm:col-span-3 text-slate-700 font-medium text-xs">上下文窗口</label>
                <div class="sm:col-span-9">
                  <div
                    class="flex flex-wrap items-center gap-1.5 p-1 bg-slate-100/90 rounded-lg select-none border border-slate-200/70"
                    data-testid="context-window-buttons"
                  >
                    <button
                      v-for="p in CONTEXT_PRESETS"
                      :key="p"
                      type="button"
                      :data-testid="`context-btn-${p}`"
                      class="flex-1 py-1 px-2 rounded-md text-xs font-medium transition-all cursor-pointer text-center"
                      :class="[
                        !isCustomContext && form.context_window?.toLowerCase() === p.toLowerCase()
                          ? 'bg-slate-900 text-white shadow-2xs font-semibold'
                          : 'text-slate-600 hover:text-slate-900 hover:bg-white/50'
                      ]"
                      @click="setContextPreset(p)"
                    >
                      {{ p.toUpperCase() }}
                    </button>
                    <button
                      type="button"
                      data-testid="context-btn-custom"
                      class="py-1 px-3 rounded-md text-xs font-medium transition-all cursor-pointer text-center"
                      :class="[
                        isCustomContext
                          ? 'bg-slate-900 text-white shadow-2xs font-semibold'
                          : 'text-slate-600 hover:text-slate-900 hover:bg-white/50'
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
                      class="w-full bg-white border border-slate-200/80 rounded-lg px-3 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-500 mt-1"
                      data-testid="model-context-window-input"
                    />
                  </UiCollapsible>
                </div>
              </div>

              <!-- 思考强度按钮组 (同一行2列) -->
              <div class="grid grid-cols-1 sm:grid-cols-12 gap-2 sm:gap-4 items-center">
                <label class="sm:col-span-3 text-slate-700 font-medium text-xs flex items-center gap-1">
                  <span>思考强度</span>
                  <UiTooltip content="设置模型推理思考深度的预设档位，按需开启深度认知">
                    <Icons name="info" size="12" class="text-slate-400 cursor-pointer" />
                  </UiTooltip>
                </label>
                <div class="sm:col-span-9">
                  <ThinkingEffortSelect
                    v-model:default-effort="form.thinking_default"
                    :disabled="!adminWriteEnabled"
                  />
                </div>
              </div>

              <!-- 支持模态类型拆解 (按两列分别排列) -->
              <div class="grid grid-cols-1 sm:grid-cols-12 gap-2 sm:gap-4 items-center" data-testid="model-modalities-section">
                <label class="sm:col-span-3 text-slate-700 font-medium text-xs">模态支持</label>
                <div class="sm:col-span-9 grid grid-cols-1 sm:grid-cols-2 gap-3">
                  <!-- 输入模态列 -->
                  <div class="flex items-center justify-between p-2.5 bg-slate-100/70 rounded-lg border border-slate-200/60" data-testid="model-input-modalities">
                    <span class="text-xs text-slate-700 font-medium select-none">输入:</span>
                    <div class="flex items-center gap-1.5">
                      <UiTooltip
                        v-for="m in MODALITIES"
                        :key="`in-${m.id}`"
                        :content="`输入支持: ${m.label} (点击切换)`"
                      >
                        <button
                          type="button"
                          :data-testid="`input-modality-btn-${m.id}`"
                          class="h-7 w-7 rounded-md flex items-center justify-center transition-all cursor-pointer"
                          :class="[
                            form.input_types.includes(m.id)
                              ? 'bg-slate-900 text-white shadow-2xs font-semibold'
                              : 'bg-white text-slate-400 hover:text-slate-700 hover:bg-slate-50 border border-slate-200/60'
                          ]"
                          @click="toggleInputModality(m.id)"
                        >
                          <Icons :name="m.icon" size="14" />
                        </button>
                      </UiTooltip>
                    </div>
                  </div>

                  <!-- 输出模态列 -->
                  <div class="flex items-center justify-between p-2.5 bg-slate-100/70 rounded-lg border border-slate-200/60" data-testid="model-output-modalities">
                    <span class="text-xs text-slate-700 font-medium select-none">输出:</span>
                    <div class="flex items-center gap-1.5">
                      <UiTooltip
                        v-for="m in MODALITIES"
                        :key="`out-${m.id}`"
                        :content="`输出支持: ${m.label} (点击切换)`"
                      >
                        <button
                          type="button"
                          :data-testid="`output-modality-btn-${m.id}`"
                          class="h-7 w-7 rounded-md flex items-center justify-center transition-all cursor-pointer"
                          :class="[
                            form.output_types.includes(m.id)
                              ? 'bg-slate-900 text-white shadow-2xs font-semibold'
                              : 'bg-white text-slate-400 hover:text-slate-700 hover:bg-slate-50 border border-slate-200/60'
                          ]"
                          @click="toggleOutputModality(m.id)"
                        >
                          <Icons :name="m.icon" size="14" />
                        </button>
                      </UiTooltip>
                    </div>
                  </div>
                </div>
              </div>

              <!-- 下级折叠层级：标题直接是“高级” -->
              <div class="pt-1">
                <button
                  type="button"
                  class="text-xs font-semibold text-slate-600 hover:text-slate-900 inline-flex items-center gap-1 cursor-pointer py-1 select-none"
                  data-testid="toggle-advanced-btn"
                  @click="showAdvanced = !showAdvanced"
                >
                  <Icons :name="showAdvanced ? 'chevron-down' : 'chevron-right'" size="12" />
                  高级
                </button>

                <UiCollapsible :open="showAdvanced">
                  <div class="p-3.5 bg-slate-100/70 backdrop-blur-xs rounded-lg space-y-3.5 mt-1.5 border border-slate-200/60">
                    <!-- 3 种底层协议选项 (同一行2列) -->
                    <div class="grid grid-cols-1 sm:grid-cols-12 gap-2 sm:gap-4 items-center">
                      <label class="sm:col-span-3 text-slate-600 font-medium text-xs">协议覆盖</label>
                      <div class="sm:col-span-9">
                        <div class="flex flex-wrap gap-1.5" data-testid="model-protocol-options">
                          <button
                            v-for="opt in PROTOCOL_OPTIONS"
                            :key="opt.value"
                            type="button"
                            :data-testid="`model-proto-${opt.value}`"
                            class="px-2.5 py-1 rounded-md text-xs font-medium transition-all cursor-pointer border"
                            :class="[
                              form.protocol === opt.value
                                ? 'bg-slate-900 text-white border-slate-900 shadow-2xs font-semibold'
                                : 'bg-white text-slate-600 border-slate-200 hover:border-slate-300'
                            ]"
                            @click="selectProtocol(opt.value)"
                          >
                            {{ opt.label }}
                          </button>
                        </div>
                        <p class="text-3xs text-slate-400 mt-1">未选中时继承服务商默认协议；点击已选协议可取消选择。</p>
                      </div>
                    </div>

                    <!-- 专属 base_url 输入框 (同一行2列) -->
                    <div class="grid grid-cols-1 sm:grid-cols-12 gap-2 sm:gap-4 items-center">
                      <label class="sm:col-span-3 text-slate-600 font-medium text-xs">Base URL</label>
                      <div class="sm:col-span-9">
                        <input
                          v-model="form.base_url"
                          type="text"
                          placeholder="例如: https://api.openai.com/v1 或留空继承服务商配置"
                          class="w-full bg-white border border-slate-200/80 rounded-lg px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-500"
                          data-testid="model-base-url-input"
                        />
                      </div>
                    </div>

                    <!-- 默认采样参数 (请求未传时生效，同一行2列) -->
                    <div class="grid grid-cols-1 sm:grid-cols-12 gap-2 sm:gap-4 items-center">
                      <label class="sm:col-span-3 text-slate-600 font-medium text-xs">默认采样</label>
                      <div class="sm:col-span-9 grid grid-cols-2 gap-2">
                        <div>
                          <input
                            v-model="form.temperature"
                            type="text"
                            inputmode="decimal"
                            placeholder="Temperature 如 0.7"
                            class="w-full bg-white border border-slate-200/80 rounded-lg px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-500"
                            data-testid="model-temperature-input"
                          />
                        </div>
                        <div>
                          <input
                            v-model="form.top_p"
                            type="text"
                            inputmode="decimal"
                            placeholder="Top P 如 0.9"
                            class="w-full bg-white border border-slate-200/80 rounded-lg px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-500"
                            data-testid="model-top-p-input"
                          />
                        </div>
                      </div>
                    </div>

                    <!-- 价格与峰谷模式 (同一行2列：默认价格为谷价，可添加峰价时段) -->
                    <div class="grid grid-cols-1 sm:grid-cols-12 gap-2 sm:gap-4 items-start pt-1">
                      <div class="sm:col-span-3 text-slate-600 font-medium text-xs pt-1.5">
                        <span>价格</span>
                        <p class="text-3xs text-slate-400 font-normal mt-0.5">￥/M tokens</p>
                      </div>
                      <div class="sm:col-span-9 space-y-3">
                        <!-- 默认价格 (谷价) 基础价格卡片 -->
                        <div class="p-3 bg-slate-100/60 rounded-lg border border-slate-200/60">
                          <div class="flex items-center justify-between mb-2">
                            <span class="text-xs font-semibold text-slate-800 flex items-center gap-1">
                              <span>默认价格（谷价）</span>
                              <UiTooltip content="未进入峰价特别时段或非峰价周末时，统一按此单价计费">
                                <Icons name="info" size="12" class="text-slate-400 cursor-pointer" />
                              </UiTooltip>
                            </span>
                            <span class="text-3xs text-slate-400 font-mono">常规/非高峰</span>
                          </div>
                          <div class="grid grid-cols-3 gap-2.5">
                            <div>
                              <label class="block text-slate-500 mb-0.5 text-3xs">输入单价</label>
                              <input
                                v-model="form.input_price"
                                type="text"
                                inputmode="decimal"
                                placeholder="0.00"
                                class="w-full bg-white border border-slate-200/80 rounded px-2.5 py-1 text-xs text-slate-800 font-mono focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-500"
                                data-testid="model-input-price-input"
                              />
                            </div>
                            <div>
                              <label class="block text-slate-500 mb-0.5 text-3xs">缓存单价</label>
                              <input
                                v-model="form.cached_price"
                                type="text"
                                inputmode="decimal"
                                placeholder="0.00"
                                class="w-full bg-white border border-slate-200/80 rounded px-2.5 py-1 text-xs text-slate-800 font-mono focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-500"
                                data-testid="model-cached-price-input"
                              />
                            </div>
                            <div>
                              <label class="block text-slate-500 mb-0.5 text-3xs">输出单价</label>
                              <input
                                v-model="form.output_price"
                                type="text"
                                inputmode="decimal"
                                placeholder="0.00"
                                class="w-full bg-white border border-slate-200/80 rounded px-2.5 py-1 text-xs text-slate-800 font-mono focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-500"
                                data-testid="model-output-price-input"
                              />
                            </div>
                          </div>
                        </div>

                        <!-- 峰价特别时段列表与添加按钮 (无空白行，用户点击添加时增加) -->
                        <div class="space-y-2.5">
                          <div v-if="form.pricing_periods.length > 0" class="flex items-center justify-between text-xs text-slate-700 font-medium">
                            <span class="flex items-center gap-1">
                              <span class="w-1.5 h-1.5 rounded-full bg-amber-500"></span>
                              峰价特别时段
                            </span>
                            <span class="text-3xs text-slate-400">仅在指定时段执行以下单价</span>
                          </div>

                          <div
                            v-for="(period, pIdx) in form.pricing_periods"
                            :key="`period-${pIdx}`"
                            class="p-2.5 bg-white/80 rounded-lg border border-slate-200/70 text-xs space-y-2"
                          >
                            <div class="flex flex-wrap items-center gap-2.5 justify-between">
                              <!-- 0-23整点时段选择器与包含周末勾选 -->
                              <div class="flex items-center gap-2 shrink-0">
                                <span class="text-xs text-amber-700 font-medium bg-amber-50 px-1.5 py-0.5 rounded border border-amber-200/60">峰时</span>
                                <HourRangePicker
                                  v-model:start-time="period.start_time"
                                  v-model:end-time="period.end_time"
                                />
                                <label class="inline-flex items-center gap-1 text-xs text-slate-600 select-none cursor-pointer">
                                  <input
                                    v-model="period.include_weekends"
                                    type="checkbox"
                                    class="rounded border-slate-300 text-slate-900 focus:ring-0 cursor-pointer"
                                  />
                                  <span>包含周末</span>
                                </label>
                              </div>

                              <!-- 紧凑单价输入框 (输入、缓存、输出单价并排一行) -->
                              <div class="flex items-center gap-2.5 grow sm:grow-0 justify-end">
                                <div class="flex items-center gap-1">
                                  <span class="text-3xs text-slate-500 shrink-0 font-medium">输入:</span>
                                  <input
                                    v-model="period.input_price"
                                    type="text"
                                    inputmode="decimal"
                                    placeholder="0.00"
                                    class="w-16 bg-white border border-slate-200 rounded px-1.5 py-1 text-xs text-slate-800 font-mono text-right"
                                  />
                                </div>
                                <div class="flex items-center gap-1">
                                  <span class="text-3xs text-slate-500 shrink-0 font-medium">缓存:</span>
                                  <input
                                    v-model="period.cached_price"
                                    type="text"
                                    inputmode="decimal"
                                    placeholder="0.00"
                                    class="w-16 bg-white border border-slate-200 rounded px-1.5 py-1 text-xs text-slate-800 font-mono text-right"
                                  />
                                </div>
                                <div class="flex items-center gap-1">
                                  <span class="text-3xs text-slate-500 shrink-0 font-medium">输出:</span>
                                  <input
                                    v-model="period.output_price"
                                    type="text"
                                    inputmode="decimal"
                                    placeholder="0.00"
                                    class="w-16 bg-white border border-slate-200 rounded px-1.5 py-1 text-xs text-slate-800 font-mono text-right"
                                  />
                                </div>

                                <button
                                  type="button"
                                  class="text-slate-400 hover:text-rose-600 p-1 cursor-pointer ml-0.5 shrink-0"
                                  title="删除峰价时段"
                                  @click="removePricingPeriod(pIdx)"
                                >
                                  <Icons name="trash" size="13" />
                                </button>
                              </div>
                            </div>
                          </div>

                          <button
                            type="button"
                            class="w-full py-1.5 border border-dashed border-slate-300 hover:border-slate-400 rounded-lg text-xs text-slate-600 hover:text-slate-900 flex items-center justify-center gap-1 cursor-pointer bg-slate-50/50"
                            @click="addPricingPeriod"
                          >
                            <Icons name="plus" size="13" />
                            添加峰价特别时段
                          </button>
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
