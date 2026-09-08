<script setup lang="ts">
import { ref } from 'vue';
import type { ModelView, CreateModelPayload, UpdateModelPayload } from '../../types/admin';
import Icons from '../ui/Icons.vue';
import UiButton from '../ui/UiButton.vue';
import UiBadge from '../ui/UiBadge.vue';
import UiTooltip from '../ui/UiTooltip.vue';
import UiCollapsible from '../ui/UiCollapsible.vue';
import ThinkingEffortSelect from './ThinkingEffortSelect.vue';

const props = defineProps<{
  providerName: string;
  models: ModelView[];
  adminWriteEnabled: boolean;
}>();

const emit = defineEmits<{
  (e: 'create', payload: CreateModelPayload): Promise<void>;
  (e: 'update', name: string, payload: UpdateModelPayload): Promise<void>;
  (e: 'delete', name: string): Promise<void>;
}>();

const MODEL_TIERS = [
  { value: 'Smart', label: '主力 (Standard)' },
  { value: 'Large', label: '旗舰 (Flagship)' },
  { value: 'Fast', label: '轻量 (Light)' },
] as const;

const CONTEXT_PRESETS = ['8k', '32k', '128k', '1m'] as const;

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
  tier: 'Smart',
  context_window: '128k',
  thinking_default: 'Off',
  modalities: ['text', 'image'] as string[],
  protocol: '',
  input_price: 0,
  cached_price: 0,
  output_price: 0,
});

function openAddInline() {
  editingModelName.value = null;
  form.value = {
    name: '',
    tier: 'Smart',
    context_window: '128k',
    thinking_default: 'Off',
    modalities: ['text', 'image'],
    protocol: '',
    input_price: 0,
    cached_price: 0,
    output_price: 0,
  };
  isCustomContext.value = false;
  formError.value = null;
  showAdvanced.value = false;
  isAdding.value = true;
}

function openEditInline(model: ModelView) {
  isAdding.value = false;
  editingModelName.value = model.name;
  const cw = model.context_window || '128k';
  const isPreset = (CONTEXT_PRESETS as readonly string[]).includes(cw.toLowerCase());
  isCustomContext.value = !isPreset;

  form.value = {
    name: model.name,
    tier: model.tier || 'Smart',
    context_window: cw,
    thinking_default: model.thinking_default || 'Off',
    modalities: ['text', 'image'],
    protocol: model.protocol || '',
    input_price: 0,
    cached_price: 0,
    output_price: 0,
  };
  formError.value = null;
  showAdvanced.value = Boolean(model.protocol);
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

function toggleModality(mod: string) {
  const idx = form.value.modalities.indexOf(mod);
  if (idx >= 0) {
    if (form.value.modalities.length > 1) {
      form.value.modalities.splice(idx, 1);
    }
  } else {
    form.value.modalities.push(mod);
  }
}

async function handleSubmit() {
  const name = form.value.name.trim();
  if (!name) {
    formError.value = '请输入模型名称';
    return;
  }

  submitting.value = true;
  formError.value = null;
  try {
    const payloadData = {
      provider: props.providerName,
      tier: form.value.tier,
      context_window: form.value.context_window,
      thinking_default: form.value.thinking_default,
      thinking_max: form.value.thinking_default === 'Off' ? 'Off' : 'High',
      protocol: form.value.protocol ? form.value.protocol.trim() : null,
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
    <!-- 标题与快捷添加按钮 -->
    <div class="flex items-center justify-between pb-1">
      <div class="flex items-center gap-1.5 text-xs font-semibold text-slate-700">
        <Icons name="sparkles" size="14" class="text-indigo-500" />
        挂载模型 ({{ models.length }})
        <UiTooltip content="该服务商对外暴露的可路由模型字典及其上下文与思考强度参数">
          <Icons name="info" size="12" class="text-slate-400 cursor-pointer" />
        </UiTooltip>
      </div>

      <UiButton
        variant="ghost"
        size="sm"
        :disabled="!adminWriteEnabled || isAdding"
        data-testid="add-model-btn"
        class="text-indigo-600 hover:text-indigo-700 hover:bg-indigo-50/60 font-medium px-2.5 py-1 text-xs"
        @click="openAddInline"
      >
        <Icons name="plus" size="13" />
        模型
      </UiButton>
    </div>

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
          <!-- 模型名称输入 -->
          <div>
            <label class="block text-slate-600 font-medium mb-1 text-xs">模型名称 *</label>
            <input
              v-model="form.name"
              type="text"
              placeholder="例如: gpt-4o 或 deepseek-v3"
              required
              class="w-full bg-white border border-slate-200/80 rounded-lg px-3 py-2 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500"
              data-testid="model-name-input"
            />
          </div>

          <!-- 模型分级 (Tier) 按钮选项组 -->
          <div>
            <label class="block text-slate-600 font-medium mb-1.5 text-xs">模型分级 (Tier)</label>
            <div
              class="grid grid-cols-3 gap-1.5 p-1 bg-slate-200/60 rounded-lg select-none"
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
                    ? 'bg-white text-indigo-700 shadow-2xs font-semibold'
                    : 'text-slate-600 hover:text-slate-900 hover:bg-white/40'
                ]"
                @click="form.tier = t.value"
              >
                {{ t.label }}
              </button>
            </div>
          </div>

          <!-- 上下文窗口 (Context Window) 按钮选项组 -->
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
                  !isCustomContext && form.context_window === p
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

          <!-- 输入输出多模态类型 (仅纯图标语义按钮) -->
          <div>
            <label class="block text-slate-600 font-medium mb-1.5 text-xs flex items-center gap-1.5">
              <span>支持模态类型</span>
              <UiTooltip content="点击纯图标切换该模型支持的输入/输出模态能力">
                <Icons name="info" size="12" class="text-slate-400 cursor-pointer" />
              </UiTooltip>
            </label>
            <div class="flex items-center gap-2" data-testid="model-modalities-group">
              <UiTooltip
                v-for="m in MODALITIES"
                :key="m.id"
                :content="`支持${m.label}输入/输出 (点击切换)`"
              >
                <button
                  type="button"
                  :data-testid="`modality-btn-${m.id}`"
                  class="h-9 w-9 rounded-lg flex items-center justify-center transition-all cursor-pointer"
                  :class="[
                    form.modalities.includes(m.id)
                      ? 'bg-indigo-50 text-indigo-600 ring-1 ring-indigo-200 shadow-2xs'
                      : 'bg-slate-100 text-slate-400 hover:text-slate-600 hover:bg-slate-200/60'
                  ]"
                  @click="toggleModality(m.id)"
                >
                  <Icons :name="m.icon" size="16" />
                </button>
              </UiTooltip>
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
              <div class="p-3 bg-slate-100/70 rounded-lg space-y-2.5 mt-1.5">
                <div>
                  <label class="block text-slate-500 font-medium mb-1 text-3xs">计费单价参考 (USD / 1M Tokens)</label>
                  <div class="grid grid-cols-3 gap-2">
                    <div>
                      <span class="block text-slate-400 text-3xs mb-0.5">输入</span>
                      <input
                        v-model.number="form.input_price"
                        type="number"
                        step="0.001"
                        placeholder="0.00"
                        class="w-full bg-white border border-slate-200 rounded px-2 py-1 text-xs text-slate-800"
                      />
                    </div>
                    <div>
                      <span class="block text-slate-400 text-3xs mb-0.5">缓存</span>
                      <input
                        v-model.number="form.cached_price"
                        type="number"
                        step="0.001"
                        placeholder="0.00"
                        class="w-full bg-white border border-slate-200 rounded px-2 py-1 text-xs text-slate-800"
                      />
                    </div>
                    <div>
                      <span class="block text-slate-400 text-3xs mb-0.5">输出</span>
                      <input
                        v-model.number="form.output_price"
                        type="number"
                        step="0.001"
                        placeholder="0.00"
                        class="w-full bg-white border border-slate-200 rounded px-2 py-1 text-xs text-slate-800"
                      />
                    </div>
                  </div>
                </div>

                <div>
                  <label class="block text-slate-500 font-medium mb-1 text-3xs">底层协议覆盖 (选填)</label>
                  <input
                    v-model="form.protocol"
                    type="text"
                    placeholder="如: openai 或 anthropic"
                    class="w-full bg-white border border-slate-200 rounded px-2.5 py-1 text-xs text-slate-800"
                  />
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
              :disabled="submitting"
              data-testid="submit-model-btn"
            >
              {{ submitting ? '保存中...' : '保存模型' }}
            </UiButton>
          </div>
        </form>
      </div>
    </UiCollapsible>

    <!-- 模型条目列表 -->
    <div v-if="models.length === 0" class="py-3 text-center text-xs text-slate-400 bg-slate-50/50 rounded-lg">
      暂未注册模型，点击上方「+ 模型」快速挂载
    </div>

    <div v-else class="space-y-2">
      <div
        v-for="m in models"
        :key="m.name"
        class="bg-slate-50/70 hover:bg-slate-100/70 rounded-xl transition-colors text-xs overflow-hidden"
        data-testid="model-row"
      >
        <!-- 一等常显行 -->
        <div class="flex items-center justify-between px-3.5 py-2.5">
          <div class="flex items-center gap-2.5 min-w-0">
            <span class="font-semibold text-slate-800 text-sm truncate">{{ m.name }}</span>
            <UiBadge :variant="getTierBadgeVariant(m.tier)">
              {{ m.tier }}
            </UiBadge>
            <span class="text-slate-400 text-xs font-mono">{{ m.context_window }}</span>

            <!-- 多模态简明纯图标常显指示 -->
            <div class="hidden sm:flex items-center gap-1 text-slate-400">
              <UiTooltip content="支持文本模态">
                <Icons name="file-text" size="12" class="text-slate-500" />
              </UiTooltip>
              <UiTooltip content="支持图像理解">
                <Icons name="image" size="12" class="text-slate-500" />
              </UiTooltip>
            </div>

            <!-- 思考强度简明标记 (当非 Off 时展示微标) -->
            <UiTooltip
              v-if="m.thinking_default && m.thinking_default !== 'Off'"
              :content="`思考强度预设: ${m.thinking_default}`"
            >
              <span class="inline-flex items-center gap-1 text-xs text-indigo-600 bg-indigo-50/90 px-2 py-0.5 rounded-full font-medium cursor-help">
                <Icons name="brain" size="11" />
                {{ m.thinking_default }}
              </span>
            </UiTooltip>
          </div>

          <div class="flex items-center gap-1.5 shrink-0">
            <!-- 编辑纯图标按钮 -->
            <UiTooltip content="编辑模型参数">
              <UiButton
                variant="ghost"
                size="icon"
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
            <div class="flex items-center justify-between">
              <span class="font-semibold text-slate-800 text-sm">编辑模型: {{ m.name }}</span>
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
              <!-- 模型分级 (Tier) 按钮选项组 -->
              <div>
                <label class="block text-slate-600 font-medium mb-1.5 text-xs">模型分级 (Tier)</label>
                <div
                  class="grid grid-cols-3 gap-1.5 p-1 bg-slate-100 rounded-lg select-none"
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
                        ? 'bg-white text-indigo-700 shadow-2xs font-semibold'
                        : 'text-slate-600 hover:text-slate-900 hover:bg-white/40'
                    ]"
                    @click="form.tier = t.value"
                  >
                    {{ t.label }}
                  </button>
                </div>
              </div>

              <!-- 上下文窗口 (Context Window) 按钮选项组 -->
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
                      !isCustomContext && form.context_window === p
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

              <!-- 输入输出多模态类型 (仅纯图标语义按钮) -->
              <div>
                <label class="block text-slate-600 font-medium mb-1.5 text-xs flex items-center gap-1.5">
                  <span>支持模态类型</span>
                  <UiTooltip content="点击纯图标切换该模型支持的输入/输出模态能力">
                    <Icons name="info" size="12" class="text-slate-400 cursor-pointer" />
                  </UiTooltip>
                </label>
                <div class="flex items-center gap-2" data-testid="model-modalities-group">
                  <UiTooltip
                    v-for="m in MODALITIES"
                    :key="m.id"
                    :content="`支持${m.label}输入/输出 (点击切换)`"
                  >
                    <button
                      type="button"
                      :data-testid="`modality-btn-${m.id}`"
                      class="h-9 w-9 rounded-lg flex items-center justify-center transition-all cursor-pointer"
                      :class="[
                        form.modalities.includes(m.id)
                          ? 'bg-indigo-50 text-indigo-600 ring-1 ring-indigo-200 shadow-2xs'
                          : 'bg-slate-100 text-slate-400 hover:text-slate-600 hover:bg-slate-200/60'
                      ]"
                      @click="toggleModality(m.id)"
                    >
                      <Icons :name="m.icon" size="16" />
                    </button>
                  </UiTooltip>
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
                  <div class="p-3 bg-slate-100/70 rounded-lg space-y-2.5 mt-1.5">
                    <div>
                      <label class="block text-slate-500 font-medium mb-1 text-3xs">计费单价参考 (USD / 1M Tokens)</label>
                      <div class="grid grid-cols-3 gap-2">
                        <div>
                          <span class="block text-slate-400 text-3xs mb-0.5">输入</span>
                          <input
                            v-model.number="form.input_price"
                            type="number"
                            step="0.001"
                            placeholder="0.00"
                            class="w-full bg-white border border-slate-200 rounded px-2 py-1 text-xs text-slate-800"
                          />
                        </div>
                        <div>
                          <span class="block text-slate-400 text-3xs mb-0.5">缓存</span>
                          <input
                            v-model.number="form.cached_price"
                            type="number"
                            step="0.001"
                            placeholder="0.00"
                            class="w-full bg-white border border-slate-200 rounded px-2 py-1 text-xs text-slate-800"
                          />
                        </div>
                        <div>
                          <span class="block text-slate-400 text-3xs mb-0.5">输出</span>
                          <input
                            v-model.number="form.output_price"
                            type="number"
                            step="0.001"
                            placeholder="0.00"
                            class="w-full bg-white border border-slate-200 rounded px-2 py-1 text-xs text-slate-800"
                          />
                        </div>
                      </div>
                    </div>

                    <div>
                      <label class="block text-slate-500 font-medium mb-1 text-3xs">底层协议覆盖 (选填)</label>
                      <input
                        v-model="form.protocol"
                        type="text"
                        placeholder="如: openai 或 anthropic"
                        class="w-full bg-white border border-slate-200 rounded px-2.5 py-1 text-xs text-slate-800"
                      />
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
                  :disabled="submitting"
                  data-testid="submit-model-btn"
                >
                  {{ submitting ? '保存中...' : '更新模型' }}
                </UiButton>
              </div>
            </form>
          </div>
        </UiCollapsible>
      </div>
    </div>
  </div>
</template>
