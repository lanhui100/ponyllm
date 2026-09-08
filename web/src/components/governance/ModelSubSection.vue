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

const MODEL_TIERS = ['Fast', 'Smart', 'Large', 'Fallback'] as const;

const isAdding = ref(false);
const editingModelName = ref<string | null>(null);
const submitting = ref(false);
const formError = ref<string | null>(null);
const showAdvanced = ref(false);

const form = ref({
  name: '',
  tier: 'Smart',
  context_window: '128k',
  thinking_default: 'Off',
  thinking_max: 'High',
  protocol: '',
});

function openAddInline() {
  editingModelName.value = null;
  form.value = {
    name: '',
    tier: 'Smart',
    context_window: '128k',
    thinking_default: 'Off',
    thinking_max: 'High',
    protocol: '',
  };
  formError.value = null;
  showAdvanced.value = false;
  isAdding.value = true;
}

function openEditInline(model: ModelView) {
  isAdding.value = false;
  editingModelName.value = model.name;
  form.value = {
    name: model.name,
    tier: model.tier || 'Smart',
    context_window: model.context_window || '128k',
    thinking_default: model.thinking_default || 'Off',
    thinking_max: model.thinking_max || 'High',
    protocol: model.protocol || '',
  };
  formError.value = null;
  showAdvanced.value = Boolean(
    (model.thinking_default && model.thinking_default !== 'Off') ||
    (model.thinking_max && model.thinking_max !== 'High')
  );
}

function cancelForm() {
  isAdding.value = false;
  editingModelName.value = null;
  formError.value = null;
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
    if (editingModelName.value) {
      await emit('update', editingModelName.value, {
        provider: props.providerName,
        tier: form.value.tier,
        context_window: form.value.context_window,
        thinking_default: form.value.thinking_default,
        thinking_max: form.value.thinking_max,
        protocol: form.value.protocol || null,
      });
    } else {
      await emit('create', {
        name,
        provider: props.providerName,
        tier: form.value.tier,
        context_window: form.value.context_window,
        thinking_default: form.value.thinking_default,
        thinking_max: form.value.thinking_max,
        protocol: form.value.protocol || null,
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
        <Icons name="sparkles" size="13" class="text-indigo-500" />
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
        class="text-blue-600 hover:text-blue-700 hover:bg-blue-50/60 font-medium px-2 py-0.5 text-xs"
        @click="openAddInline"
      >
        <Icons name="plus" size="12" />
        模型
      </UiButton>
    </div>

    <!-- 行内平滑展开新建模型表单 -->
    <UiCollapsible :open="isAdding">
      <div class="p-3 bg-slate-50/90 rounded-lg border border-slate-200/80 mb-2.5 text-xs">
        <div class="flex items-center justify-between mb-2">
          <span class="font-medium text-slate-800">新建模型配置</span>
          <button
            type="button"
            class="text-slate-400 hover:text-slate-600 cursor-pointer"
            @click="cancelForm"
          >
            <Icons name="cross" size="13" />
          </button>
        </div>

        <div v-if="formError" class="p-2 mb-2 bg-rose-50 text-rose-600 rounded text-xs">
          {{ formError }}
        </div>

        <form class="space-y-2.5" @submit.prevent="handleSubmit">
          <div class="grid grid-cols-1 sm:grid-cols-3 gap-2">
            <div>
              <label class="block text-slate-500 mb-1">模型名称 *</label>
              <input
                v-model="form.name"
                type="text"
                placeholder="例如: gpt-4o"
                required
                class="w-full bg-white border border-slate-200 rounded px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-1 focus:ring-blue-500"
                data-testid="model-name-input"
              />
            </div>

            <div>
              <label class="block text-slate-500 mb-1">分级 Tier</label>
              <select
                v-model="form.tier"
                class="w-full bg-white border border-slate-200 rounded px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-1 focus:ring-blue-500"
                data-testid="model-tier-select"
              >
                <option v-for="t in MODEL_TIERS" :key="t" :value="t">{{ t }}</option>
              </select>
            </div>

            <div>
              <label class="block text-slate-500 mb-1">上下文窗口</label>
              <input
                v-model="form.context_window"
                type="text"
                placeholder="128k"
                class="w-full bg-white border border-slate-200 rounded px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-1 focus:ring-blue-500"
                data-testid="model-context-window-input"
              />
            </div>
          </div>

          <!-- 高级选项折叠 (含思考强度映射) -->
          <div>
            <button
              type="button"
              class="text-2xs text-slate-500 hover:text-slate-700 inline-flex items-center gap-1 cursor-pointer py-0.5"
              @click="showAdvanced = !showAdvanced"
            >
              <Icons :name="showAdvanced ? 'chevron-down' : 'chevron-right'" size="10" />
              进阶选项 (思考强度映射与协议)
            </button>

            <UiCollapsible :open="showAdvanced">
              <div class="pt-2">
                <ThinkingEffortSelect
                  v-model:default-effort="form.thinking_default"
                  v-model:max-effort="form.thinking_max"
                />
              </div>
            </UiCollapsible>
          </div>

          <div class="flex items-center justify-end gap-2 pt-1 border-t border-slate-200/60">
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

    <div v-else class="space-y-1.5">
      <div
        v-for="m in models"
        :key="m.name"
        class="bg-slate-50/70 hover:bg-slate-100/60 rounded-lg transition-colors text-xs overflow-hidden"
        data-testid="model-row"
      >
        <!-- 一等常显行 -->
        <div class="flex items-center justify-between px-3 py-2">
          <div class="flex items-center gap-2 min-w-0">
            <span class="font-medium text-slate-800 truncate">{{ m.name }}</span>
            <UiBadge :variant="getTierBadgeVariant(m.tier)">
              {{ m.tier }}
            </UiBadge>
            <span class="text-slate-400 text-2xs">{{ m.context_window }}</span>

            <!-- 思考强度简明标记 (当非默认关时展示微标) -->
            <UiTooltip v-if="m.thinking_max && m.thinking_max !== 'Off'" :content="`思考强度: 默认 ${m.thinking_default || 'Off'} / 上限 ${m.thinking_max}`">
              <span class="inline-flex items-center gap-0.5 text-2xs text-indigo-600 bg-indigo-50/80 px-1.5 py-0.2 rounded font-medium cursor-help">
                <Icons name="brain" size="10" />
                {{ m.thinking_max }}
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
                class="text-slate-500 hover:text-blue-600 hover:bg-blue-50"
                @click="openEditInline(m)"
              >
                <Icons name="edit" size="13" />
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
                <Icons name="trash" size="13" />
              </UiButton>
            </UiTooltip>
          </div>
        </div>

        <!-- 当前模型的平滑内联编辑区 -->
        <UiCollapsible :open="editingModelName === m.name">
          <div class="p-3 bg-white border-t border-slate-200/80 text-xs">
            <div class="flex items-center justify-between mb-2">
              <span class="font-medium text-slate-800">编辑模型: {{ m.name }}</span>
              <button
                type="button"
                class="text-slate-400 hover:text-slate-600 cursor-pointer"
                @click="cancelForm"
              >
                <Icons name="cross" size="13" />
              </button>
            </div>

            <div v-if="formError" class="p-2 mb-2 bg-rose-50 text-rose-600 rounded text-xs">
              {{ formError }}
            </div>

            <form class="space-y-2.5" @submit.prevent="handleSubmit">
              <div class="grid grid-cols-1 sm:grid-cols-2 gap-2">
                <div>
                  <label class="block text-slate-500 mb-1">分级 Tier</label>
                  <select
                    v-model="form.tier"
                    class="w-full bg-white border border-slate-200 rounded px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-1 focus:ring-blue-500"
                    data-testid="model-tier-select"
                  >
                    <option v-for="t in MODEL_TIERS" :key="t" :value="t">{{ t }}</option>
                  </select>
                </div>

                <div>
                  <label class="block text-slate-500 mb-1">上下文窗口</label>
                  <input
                    v-model="form.context_window"
                    type="text"
                    placeholder="128k"
                    class="w-full bg-white border border-slate-200 rounded px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-1 focus:ring-blue-500"
                    data-testid="model-context-window-input"
                  />
                </div>
              </div>

              <!-- 思考强度映射编辑 -->
              <div>
                <ThinkingEffortSelect
                  v-model:default-effort="form.thinking_default"
                  v-model:max-effort="form.thinking_max"
                />
              </div>

              <div class="flex items-center justify-end gap-2 pt-1">
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
