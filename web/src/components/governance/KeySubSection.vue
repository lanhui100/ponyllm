<script setup lang="ts">
import { ref, computed } from 'vue';
import type { KeyView, KeyTestView, CreateKeyPayload } from '../../types/admin';
import Icons from '../ui/Icons.vue';
import UiButton from '../ui/UiButton.vue';
import UiBadge from '../ui/UiBadge.vue';
import UiTooltip from '../ui/UiTooltip.vue';
import UiCollapsible from '../ui/UiCollapsible.vue';
import { formatKeyState } from '../../utils/format';

const props = defineProps<{
  providerName: string;
  keys: KeyView[];
  adminWriteEnabled: boolean;
  keyTestResults: Record<string, KeyTestView>;
  testingKeyIds: Set<string>;
  defaultExpanded?: boolean;
}>();

const emit = defineEmits<{
  (e: 'create', payload: CreateKeyPayload): Promise<void>;
  (e: 'delete', id: string): Promise<void>;
  (e: 'test-single', id: string): Promise<void>;
  (e: 'oauth-antigravity', providerName: string): void;
}>();

const isAntigravity = computed(() => props.providerName.toLowerCase().includes('antigravity'));

const isExpanded = ref(props.defaultExpanded ?? false);
const isAdding = ref(false);
const showAdvanced = ref(false);
const submitting = ref(false);
const formError = ref<string | null>(null);

const form = ref<CreateKeyPayload>({
  id: '',
  provider: props.providerName,
  api_key: '',
  priority: 1,
  weight: 10,
});

function openAddInline() {
  form.value = {
    id: `key-${Date.now().toString().slice(-4)}`,
    provider: props.providerName,
    api_key: '',
    priority: 1,
    weight: 10,
  };
  formError.value = null;
  showAdvanced.value = false;
  isExpanded.value = true;
  isAdding.value = true;
}

function cancelAdd() {
  isAdding.value = false;
  formError.value = null;
}

async function handleSubmit() {
  const id = form.value.id.trim();
  const rawKey = form.value.api_key.trim();
  if (!id) {
    formError.value = '请输入密钥标识';
    return;
  }
  if (!rawKey) {
    formError.value = '请输入 API Key 明文';
    return;
  }

  submitting.value = true;
  formError.value = null;
  try {
    await emit('create', {
      ...form.value,
      id,
      api_key: rawKey,
      provider: props.providerName,
    });
    isAdding.value = false;
  } catch (err: unknown) {
    formError.value = err instanceof Error ? err.message : String(err);
  } finally {
    submitting.value = false;
  }
}

async function handleDelete(id: string) {
  if (!confirm(`确定移除密钥 "${id}" 吗？该操作将热同步连接池。`)) {
    return;
  }
  try {
    await emit('delete', id);
  } catch (err: unknown) {
    alert(`删除失败: ${err instanceof Error ? err.message : String(err)}`);
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
        <Icons name="key" size="13" class="text-amber-500" />
        密钥 ({{ keys.length }})
        <UiTooltip content="网关向该厂商转发请求所使用的 API 密钥池，支持多 Key 负载均衡">
          <Icons name="info" size="12" class="text-slate-400 cursor-pointer" />
        </UiTooltip>
      </div>

      <div class="flex items-center gap-1" @click.stop>
        <UiButton
          v-if="isAntigravity"
          variant="ghost"
          size="sm"
          :disabled="!adminWriteEnabled"
          data-testid="add-antigravity-key-btn"
          class="text-amber-600 hover:text-amber-700 hover:bg-amber-50/60 font-medium px-2 py-1 text-xs"
          @click="emit('oauth-antigravity', props.providerName)"
        >
          <Icons name="zap" size="13" />
          授权账号
        </UiButton>
        <UiButton
          variant="ghost"
          size="sm"
          :disabled="!adminWriteEnabled || isAdding"
          data-testid="add-key-btn"
          class="text-indigo-600 hover:text-indigo-700 hover:bg-indigo-50/60 font-medium px-2.5 py-1 text-xs"
          @click="openAddInline"
        >
          <Icons name="plus" size="13" />
          密钥
        </UiButton>
        <UiButton
          variant="ghost"
          size="icon"
          class="text-slate-400 hover:text-slate-600"
          data-testid="toggle-keys-btn"
          @click="isExpanded = !isExpanded"
        >
          <Icons :name="isExpanded ? 'chevron-down' : 'chevron-right'" size="14" />
        </UiButton>
      </div>
    </div>

    <!-- 可独立折叠的内容容器 (默认折叠) -->
    <UiCollapsible :open="isExpanded">
      <div class="pt-2 space-y-2">
        <!-- 行内平滑展开新建表单 -->
        <UiCollapsible :open="isAdding">
          <div class="p-4 bg-slate-50/90 rounded-xl mb-3 text-xs space-y-3">
            <div class="flex items-center justify-between">
              <span class="font-semibold text-slate-800 text-sm">新建密钥</span>
          <button
            type="button"
            class="text-slate-400 hover:text-slate-600 cursor-pointer"
            @click="cancelAdd"
          >
            <Icons name="cross" size="14" />
          </button>
        </div>

        <div v-if="formError" class="p-2.5 bg-rose-50 text-rose-600 rounded-lg text-xs font-medium">
          {{ formError }}
        </div>

        <form class="space-y-3" @submit.prevent="handleSubmit">
          <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
            <div>
              <label class="block text-slate-600 font-medium mb-1 text-xs">Key 标识 *</label>
              <input
                v-model="form.id"
                type="text"
                placeholder="例如: key-01"
                required
                class="w-full bg-white border border-slate-200/80 rounded-lg px-3 py-2 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500"
                data-testid="key-id-input"
              />
            </div>

            <div>
              <label class="block text-slate-600 font-medium mb-1 text-xs">API Key 明文 *</label>
              <input
                v-model="form.api_key"
                type="password"
                placeholder="sk-..."
                required
                class="w-full bg-white border border-slate-200/80 rounded-lg px-3 py-2 text-xs text-slate-800 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500"
                data-testid="key-secret-input"
              />
            </div>
          </div>

          <!-- 高级选项折叠 (权重/优先级) -->
          <div class="pt-0.5">
            <button
              type="button"
              class="text-xs font-semibold text-slate-600 hover:text-indigo-600 inline-flex items-center gap-1 cursor-pointer py-1 select-none"
              @click="showAdvanced = !showAdvanced"
            >
              <Icons :name="showAdvanced ? 'chevron-down' : 'chevron-right'" size="11" />
              高级调度参数 (权重与优先级)
            </button>

            <UiCollapsible :open="showAdvanced">
              <div class="grid grid-cols-2 gap-3 pt-2 p-3 bg-slate-100/70 rounded-lg mt-1">
                <div>
                  <label class="block text-3xs text-slate-500 mb-0.5 font-medium">优先级 (数值越小越优先)</label>
                  <input
                    v-model.number="form.priority"
                    type="number"
                    min="0"
                    class="w-full bg-white border border-slate-200 rounded px-2.5 py-1.5 text-xs text-slate-800"
                    data-testid="key-priority-input"
                  />
                </div>
                <div>
                  <label class="block text-3xs text-slate-500 mb-0.5 font-medium">权重 (轮询比重)</label>
                  <input
                    v-model.number="form.weight"
                    type="number"
                    min="1"
                    class="w-full bg-white border border-slate-200 rounded px-2.5 py-1.5 text-xs text-slate-800"
                    data-testid="key-weight-input"
                  />
                </div>
              </div>
            </UiCollapsible>
          </div>

          <div class="flex items-center justify-end gap-2 pt-2 border-t border-slate-200/60">
            <UiButton variant="ghost" size="sm" @click="cancelAdd">
              取消
            </UiButton>
            <UiButton
              type="submit"
              size="sm"
              :disabled="submitting"
              data-testid="submit-key-btn"
            >
              {{ submitting ? '保存中...' : '保存密钥' }}
            </UiButton>
          </div>
        </form>
      </div>
    </UiCollapsible>

    <!-- 密钥条目列表 -->
    <div v-if="keys.length === 0" class="py-3 text-center text-xs text-slate-400 bg-slate-50/50 rounded-lg">
      暂未配置密钥，点击上方「+ 密钥」快速添加
    </div>

    <div v-else class="space-y-2">
      <div
        v-for="k in keys"
        :key="k.id"
        class="bg-slate-50/70 hover:bg-slate-100/70 rounded-xl transition-colors text-xs p-3 space-y-2"
        data-testid="key-row"
      >
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-2.5 min-w-0">
            <span class="font-mono text-slate-800 font-semibold text-xs truncate">{{ k.id }}</span>
            <span class="font-mono text-slate-400 text-xs">{{ k.masked_key }}</span>
            <UiBadge :variant="k.state === 'active' ? 'success' : 'warning'">
              {{ formatKeyState(k.state) }}
            </UiBadge>
          </div>

          <div class="flex items-center gap-1.5 shrink-0">
            <!-- 拨测延迟徽标 -->
            <template v-if="keyTestResults[k.id]">
              <UiBadge
                :variant="keyTestResults[k.id].success ? 'success' : 'destructive'"
                data-testid="probe-result-badge"
              >
                {{ keyTestResults[k.id].success ? `${keyTestResults[k.id].latency_ms}ms` : '异常' }}
              </UiBadge>
            </template>
            <span v-else-if="testingKeyIds.has(k.id)" class="text-xs text-indigo-600 animate-pulse font-medium">
              测速中...
            </span>

            <!-- 测速纯图标按钮 -->
            <UiTooltip content="快速连通性与延迟测速">
              <UiButton
                variant="ghost"
                size="icon"
                :disabled="testingKeyIds.has(k.id) || !adminWriteEnabled"
                data-testid="test-single-key-btn"
                class="text-slate-500 hover:text-amber-600 hover:bg-amber-50"
                @click="emit('test-single', k.id)"
              >
                <Icons name="zap" size="14" />
              </UiButton>
            </UiTooltip>

            <!-- 删除纯图标按钮 -->
            <UiTooltip content="移除此密钥">
              <UiButton
                variant="ghost"
                size="icon"
                :disabled="!adminWriteEnabled"
                data-testid="delete-key-btn"
                class="text-slate-400 hover:text-rose-600 hover:bg-rose-50"
                @click="handleDelete(k.id)"
              >
                <Icons name="trash" size="14" />
              </UiButton>
            </UiTooltip>
          </div>
        </div>

        <!-- Antigravity 模型配额与重置时间 -->
        <div
          v-if="(keyTestResults[k.id]?.quota?.length ?? 0) > 0"
          class="pt-2 border-t border-slate-200/60"
          data-testid="antigravity-quota-container"
        >
          <div class="text-3xs font-semibold text-slate-500 uppercase tracking-wider mb-1.5 flex items-center gap-1">
            <Icons name="zap" size="11" class="text-amber-500" />
            Antigravity 模型配额与重置时间
          </div>
          <div class="grid grid-cols-1 sm:grid-cols-2 gap-2">
            <div
              v-for="q in keyTestResults[k.id].quota"
              :key="q.model_id"
              class="bg-white p-2 rounded-lg border border-slate-200/80 shadow-2xs text-3xs space-y-1"
            >
              <div class="flex items-center justify-between font-mono">
                <span class="font-medium text-slate-700 truncate mr-2">{{ q.model_id }}</span>
                <span class="font-bold" :class="q.remaining_fraction > 0.2 ? 'text-emerald-600' : 'text-rose-600'">
                  {{ Math.round(q.remaining_fraction * 100) }}%
                </span>
              </div>
              <div class="w-full bg-slate-100 rounded-full h-1 overflow-hidden">
                <div
                  class="h-full transition-all duration-300"
                  :class="q.remaining_fraction > 0.2 ? 'bg-emerald-500' : 'bg-rose-500'"
                  :style="{ width: `${Math.round(q.remaining_fraction * 100)}%` }"
                />
              </div>
              <div class="flex items-center justify-between text-slate-400 pt-0.5">
                <span>恢复时间</span>
                <span>{{ q.time_until_reset || q.reset_time_beijing || '已就绪' }}</span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
    </div>
  </UiCollapsible>
  </div>
</template>
