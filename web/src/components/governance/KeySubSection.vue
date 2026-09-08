<script setup lang="ts">
import { ref } from 'vue';
import type { KeyView, KeyTestView, CreateKeyPayload } from '../../types/admin';
import Icons from '../ui/Icons.vue';
import UiButton from '../ui/UiButton.vue';
import UiBadge from '../ui/UiBadge.vue';
import UiTooltip from '../ui/UiTooltip.vue';
import UiCollapsible from '../ui/UiCollapsible.vue';

const props = defineProps<{
  providerName: string;
  keys: KeyView[];
  adminWriteEnabled: boolean;
  keyTestResults: Record<string, KeyTestView>;
  testingKeyIds: Set<string>;
}>();

const emit = defineEmits<{
  (e: 'create', payload: CreateKeyPayload): Promise<void>;
  (e: 'delete', id: string): Promise<void>;
  (e: 'test-single', id: string): Promise<void>;
}>();

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
    <!-- 标题与快捷添加按钮 -->
    <div class="flex items-center justify-between pb-1">
      <div class="flex items-center gap-1.5 text-xs font-semibold text-slate-700">
        <Icons name="key" size="13" class="text-amber-500" />
        密钥凭证 ({{ keys.length }})
        <UiTooltip content="网关向该厂商转发请求所使用的 API 密钥池，支持多 Key 负载均衡">
          <Icons name="info" size="12" class="text-slate-400 cursor-pointer" />
        </UiTooltip>
      </div>

      <UiButton
        variant="ghost"
        size="sm"
        :disabled="!adminWriteEnabled || isAdding"
        data-testid="add-key-btn"
        class="text-blue-600 hover:text-blue-700 hover:bg-blue-50/60 font-medium px-2 py-0.5 text-xs"
        @click="openAddInline"
      >
        <Icons name="plus" size="12" />
        密钥
      </UiButton>
    </div>

    <!-- 行内平滑展开新建表单 -->
    <UiCollapsible :open="isAdding">
      <div class="p-3 bg-slate-50/90 rounded-lg border border-slate-200/80 mb-2.5 text-xs">
        <div class="flex items-center justify-between mb-2">
          <span class="font-medium text-slate-800">新建密钥凭证</span>
          <button
            type="button"
            class="text-slate-400 hover:text-slate-600 cursor-pointer"
            @click="cancelAdd"
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
              <label class="block text-slate-500 mb-1">Key 标识 *</label>
              <input
                v-model="form.id"
                type="text"
                placeholder="例如: key-01"
                required
                class="w-full bg-white border border-slate-200 rounded px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-1 focus:ring-blue-500"
                data-testid="key-id-input"
              />
            </div>

            <div>
              <label class="block text-slate-500 mb-1">API Key 明文 *</label>
              <input
                v-model="form.api_key"
                type="password"
                placeholder="sk-..."
                required
                class="w-full bg-white border border-slate-200 rounded px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-1 focus:ring-blue-500"
                data-testid="key-secret-input"
              />
            </div>
          </div>

          <!-- 高级选项折叠 (权重/优先级) -->
          <div>
            <button
              type="button"
              class="text-2xs text-slate-500 hover:text-slate-700 inline-flex items-center gap-1 cursor-pointer py-0.5"
              @click="showAdvanced = !showAdvanced"
            >
              <Icons :name="showAdvanced ? 'chevron-down' : 'chevron-right'" size="10" />
              调度参数 (权重与优先级)
            </button>

            <UiCollapsible :open="showAdvanced">
              <div class="grid grid-cols-2 gap-2 pt-2">
                <div>
                  <label class="block text-2xs text-slate-500 mb-0.5">优先级 (数值越小越优先)</label>
                  <input
                    v-model.number="form.priority"
                    type="number"
                    min="0"
                    class="w-full bg-white border border-slate-200 rounded px-2 py-1 text-xs"
                    data-testid="key-priority-input"
                  />
                </div>
                <div>
                  <label class="block text-2xs text-slate-500 mb-0.5">权重 (轮询比重)</label>
                  <input
                    v-model.number="form.weight"
                    type="number"
                    min="1"
                    class="w-full bg-white border border-slate-200 rounded px-2 py-1 text-xs"
                    data-testid="key-weight-input"
                  />
                </div>
              </div>
            </UiCollapsible>
          </div>

          <div class="flex items-center justify-end gap-2 pt-1 border-t border-slate-200/60">
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

    <div v-else class="space-y-1.5">
      <div
        v-for="k in keys"
        :key="k.id"
        class="flex items-center justify-between px-3 py-2 bg-slate-50/70 hover:bg-slate-100/60 rounded-lg transition-colors text-xs"
        data-testid="key-row"
      >
        <div class="flex items-center gap-2.5 min-w-0">
          <span class="font-mono text-slate-800 font-medium truncate">{{ k.id }}</span>
          <span class="font-mono text-slate-400 text-2xs">{{ k.masked_key }}</span>
          <UiBadge :variant="k.state === 'active' ? 'success' : 'warning'">
            {{ k.state === 'active' ? '就绪' : k.state }}
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
          <span v-else-if="testingKeyIds.has(k.id)" class="text-2xs text-blue-600 animate-pulse">
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
              <Icons name="zap" size="13" />
            </UiButton>
          </UiTooltip>

          <!-- 删除纯图标按钮 -->
          <UiTooltip content="移除此密钥凭证">
            <UiButton
              variant="ghost"
              size="icon"
              :disabled="!adminWriteEnabled"
              data-testid="delete-key-btn"
              class="text-slate-400 hover:text-rose-600 hover:bg-rose-50"
              @click="handleDelete(k.id)"
            >
              <Icons name="trash" size="13" />
            </UiButton>
          </UiTooltip>
        </div>
      </div>
    </div>
  </div>
</template>
