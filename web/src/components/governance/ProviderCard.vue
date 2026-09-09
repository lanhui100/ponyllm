<script setup lang="ts">
import { ref, computed, watch } from 'vue';
import type {
  ProviderView,
  ModelView,
  KeyView,
  KeyTestView,
  CreateModelPayload,
  UpdateModelPayload,
  CreateKeyPayload,
  UpdateProviderPayload,
} from '../../types/admin';
import Icons from '../ui/Icons.vue';
import UiButton from '../ui/UiButton.vue';
import UiBadge from '../ui/UiBadge.vue';
import UiTooltip from '../ui/UiTooltip.vue';
import UiCollapsible from '../ui/UiCollapsible.vue';
import KeySubSection from './KeySubSection.vue';
import ModelSubSection from './ModelSubSection.vue';
import { formatStrategyLabel } from '../../utils/format';

const props = defineProps<{
  provider: ProviderView;
  models: ModelView[];
  keys: KeyView[];
  adminWriteEnabled: boolean;
  keyTestResults: Record<string, KeyTestView>;
  testingKeyIds: Set<string>;
  defaultExpanded?: boolean;
  onBatchCreate?: (
    provider: string,
    ids: string[],
    onProgress: (done: number, total: number) => void,
  ) => Promise<{ added: number; skipped: number; failed: number }>;
}>();

const emit = defineEmits<{
  (e: 'delete-provider', name: string): Promise<void>;
  (e: 'update-provider', name: string, payload: UpdateProviderPayload): Promise<void>;
  (e: 'create-model', payload: CreateModelPayload): Promise<void>;
  (e: 'update-model', name: string, payload: UpdateModelPayload): Promise<void>;
  (e: 'delete-model', name: string): Promise<void>;
  (e: 'notice', message: string): void;
  (e: 'create-key', payload: CreateKeyPayload): Promise<void>;
  (e: 'delete-key', id: string): Promise<void>;
  (e: 'test-single-key', id: string): Promise<void>;
  (e: 'test-provider-keys', providerName: string): Promise<void>;
  (e: 'oauth-antigravity', providerName: string): void;
}>();

const expanded = ref(props.defaultExpanded ?? true);

const PROTOCOL_OPTIONS = [
  { id: 'chat', label: 'OpenAI Chat' },
  { id: 'messages', label: 'Anthropic Messages' },
  { id: 'responses', label: 'OpenAI Responses' },
  { id: 'antigravity', label: 'Antigravity' },
] as const;

function getInitialProtocols(): string[] {
  const list: string[] = [];
  if (
    props.provider.default_protocol === 'antigravity' ||
    props.provider.name.toLowerCase().includes('antigravity')
  ) {
    list.push('antigravity');
  }
  if (props.provider.chat_url || props.provider.default_protocol === 'chat') {
    list.push('chat');
  }
  if (
    props.provider.messages_url ||
    props.provider.default_protocol === 'messages' ||
    props.provider.default_protocol === 'anthropic'
  ) {
    list.push('messages');
  }
  if (props.provider.responses_url || props.provider.default_protocol === 'responses') {
    list.push('responses');
  }
  if (list.length === 0) {
    list.push('chat');
  }
  return list;
}

const activeProtocols = ref<string[]>(getInitialProtocols());
const customUrls = ref({
  chat: props.provider.chat_url || '',
  messages: props.provider.messages_url || '',
  responses: props.provider.responses_url || '',
});
const isEditingProtocols = ref(false);
const protocolsSaving = ref(false);

watch(
  () => props.provider,
  (p) => {
    activeProtocols.value = getInitialProtocols();
    customUrls.value = {
      chat: p.chat_url || '',
      messages: p.messages_url || '',
      responses: p.responses_url || '',
    };
  },
  { deep: true }
);

function toggleProtocol(id: string) {
  if (!isEditingProtocols.value) return;
  const idx = activeProtocols.value.indexOf(id);
  if (idx > -1) {
    if (activeProtocols.value.length > 1) {
      activeProtocols.value.splice(idx, 1);
    }
  } else {
    activeProtocols.value.push(id);
  }
}

function cancelEditProtocols() {
  activeProtocols.value = getInitialProtocols();
  customUrls.value = {
    chat: props.provider.chat_url || '',
    messages: props.provider.messages_url || '',
    responses: props.provider.responses_url || '',
  };
  isEditingProtocols.value = false;
}

async function handleSaveProtocols() {
  if (!props.adminWriteEnabled) return;
  const urlRegex = /^https?:\/\//i;
  const chatVal = customUrls.value.chat.trim();
  const messagesVal = customUrls.value.messages.trim();
  const responsesVal = customUrls.value.responses.trim();

  if (activeProtocols.value.includes('chat') && chatVal && !urlRegex.test(chatVal)) {
    alert('OpenAI Chat 专属 Base URL 必须以 http:// 或 https:// 开头');
    return;
  }
  if (activeProtocols.value.includes('messages') && messagesVal && !urlRegex.test(messagesVal)) {
    alert('Anthropic Messages 专属 Base URL 必须以 http:// 或 https:// 开头');
    return;
  }
  if (activeProtocols.value.includes('responses') && responsesVal && !urlRegex.test(responsesVal)) {
    alert('OpenAI Responses 专属 Base URL 必须以 http:// 或 https:// 开头');
    return;
  }

  protocolsSaving.value = true;
  try {
    await emit('update-provider', props.provider.name, {
      default_protocol: activeProtocols.value[0] || 'chat',
      chat_url: activeProtocols.value.includes('chat') ? chatVal : '',
      messages_url: activeProtocols.value.includes('messages') ? messagesVal : '',
      responses_url: activeProtocols.value.includes('responses') ? responsesVal : '',
    });
    isEditingProtocols.value = false;
  } catch (err: unknown) {
    alert(`保存协议配置失败: ${err instanceof Error ? err.message : String(err)}`);
  } finally {
    protocolsSaving.value = false;
  }
}

const hasCustomUrls = computed(() => {
  return Boolean(props.provider.chat_url || props.provider.messages_url || props.provider.responses_url);
});

const activeKeysCount = computed(() => {
  return props.keys.filter((k) => k.state === 'active').length;
});

async function handleDeleteProvider() {
  if (
    !confirm(
      `确定删除服务商 "${props.provider.name}" 吗？该操作将级联清理下属模型与密钥，在途请求不受影响。`
    )
  ) {
    return;
  }
  try {
    await emit('delete-provider', props.provider.name);
  } catch (err: unknown) {
    alert(`删除失败: ${err instanceof Error ? err.message : String(err)}`);
  }
}

function handleBatchTest() {
  emit('test-provider-keys', props.provider.name);
}
</script>

<template>
  <div
    class="swiss-card mb-4 overflow-hidden"
    data-testid="provider-row"
  >
    <!-- 服务商一级卡片头部 (一等常显) -->
    <div
      class="p-5 flex items-center justify-between gap-4 cursor-pointer hover:bg-slate-50/70 transition-colors select-none"
      @click="expanded = !expanded"
    >
      <!-- 左侧：厂商标识与摘要 -->
      <div class="flex items-center gap-3.5 min-w-0">
        <!-- 暖橙色图标 -->
        <div class="w-10.5 h-10.5 rounded-xl bg-orange-50 text-orange-600 border border-orange-200/60 flex items-center justify-center shrink-0 font-bold text-base shadow-2xs">
          <Icons name="server" size="22" />
        </div>

        <div class="min-w-0">
          <div class="flex items-center gap-2.5">
            <span class="font-bold text-slate-900 text-lg tracking-tight truncate">
              {{ provider.name }}
            </span>
            <UiBadge variant="secondary" class="text-[13px] font-semibold">
              {{ formatStrategyLabel(provider.strategy) }}
            </UiBadge>
          </div>

          <!-- 去除敏感明文 URL，仅在有默认模型时显示默认模型 -->
          <div v-if="provider.default_model" class="flex items-center gap-2 text-sm text-slate-500 mt-1 truncate">
            <span>默认模型: <span class="font-mono text-slate-700 font-medium">{{ provider.default_model }}</span></span>
          </div>
        </div>
      </div>

      <!-- 右侧：概览徽标与一等纯图标操作组 -->
      <div class="flex items-center gap-2.5 shrink-0" @click.stop>
        <div class="hidden sm:flex items-center gap-2 mr-2">
          <UiBadge variant="default" class="text-[13px] font-semibold">
            {{ models.length }} 模型
          </UiBadge>
          <UiBadge :variant="activeKeysCount > 0 ? 'success' : 'secondary'" class="text-[13px] font-semibold">
            {{ activeKeysCount }}/{{ keys.length }} 密钥可用
          </UiBadge>
        </div>

        <!-- ⚡ 一键测速该服务商全部 Key -->
        <UiTooltip content="一键测试该服务商下所有密钥的连通性与延迟">
          <UiButton
            variant="ghost"
            size="icon"
            aria-label="一键测试该服务商下所有密钥"
            :disabled="keys.length === 0 || !adminWriteEnabled"
            class="text-amber-500 hover:text-amber-600 hover:bg-amber-50"
            @click="handleBatchTest"
          >
            <Icons name="zap" size="15" />
          </UiButton>
        </UiTooltip>

        <!-- 🗑 删除服务商 -->
        <UiTooltip content="删除该服务商">
          <UiButton
            variant="ghost"
            size="icon"
            aria-label="删除该服务商"
            :disabled="!adminWriteEnabled"
            data-testid="delete-provider-btn"
            class="text-slate-400 hover:text-rose-600 hover:bg-rose-50"
            @click="handleDeleteProvider"
          >
            <Icons name="trash" size="15" />
          </UiButton>
        </UiTooltip>

        <!-- 展开/折叠切换 -->
        <UiButton
          variant="ghost"
          size="icon"
          :aria-label="expanded ? '收起服务商详情' : '展开服务商详情'"
          class="text-slate-400 hover:text-slate-700"
          @click="expanded = !expanded"
        >
          <Icons :name="expanded ? 'chevron-down' : 'chevron-right'" size="16" />
        </UiButton>
      </div>
    </div>

    <!-- 二级折叠展开区域 (包含模型协议、密钥及模型) -->
    <UiCollapsible :open="expanded">
      <div class="px-5 pb-5 pt-3 border-t border-slate-200/80 bg-slate-50/60 space-y-4">
        <!-- 模型协议选择器与专属端点 (同级非下拉多选) -->
        <div class="bg-white rounded-xl p-4.5 border border-slate-200/80 shadow-2xs space-y-3" data-testid="protocol-section">
          <div class="flex items-center justify-between">
            <div class="flex items-center gap-1.5 text-[14px] font-semibold text-slate-800">
              <Icons name="activity" size="16" class="text-amber-600" />
              模型协议
              <UiTooltip content="该服务商默认提供的协议与端点，未覆盖时统一走 Base URL">
                <Icons name="info" size="14" class="text-slate-400 cursor-pointer" />
              </UiTooltip>
            </div>

            <div v-if="adminWriteEnabled">
              <UiButton
                v-if="!isEditingProtocols"
                variant="ghost"
                size="sm"
                class="text-slate-700 hover:text-slate-900 hover:bg-slate-100 text-[13px] py-1"
                data-testid="edit-protocols-btn"
                @click="isEditingProtocols = true"
              >
                配置端点
              </UiButton>
              <div v-else class="flex items-center gap-1.5">
                <UiButton
                  variant="ghost"
                  size="sm"
                  class="text-[13px] py-1"
                  @click="cancelEditProtocols"
                >
                  取消
                </UiButton>
                <UiButton
                  size="sm"
                  class="text-[13px] py-1"
                  :disabled="protocolsSaving || !adminWriteEnabled"
                  data-testid="save-protocols-btn"
                  @click="handleSaveProtocols"
                >
                  {{ protocolsSaving ? '保存中...' : '保存' }}
                </UiButton>
              </div>
            </div>
          </div>

          <!-- 非下拉多选协议药丸胶囊 -->
          <div class="flex flex-wrap gap-2 items-center">
            <button
              v-for="proto in PROTOCOL_OPTIONS"
              :key="proto.id"
              type="button"
              :disabled="!isEditingProtocols"
              :data-testid="`protocol-pill-${proto.id}`"
              class="px-3 py-1.5 rounded-lg text-[13px] font-medium transition-all inline-flex items-center gap-2 border select-none"
              :class="[
                activeProtocols.includes(proto.id)
                  ? 'bg-slate-900 text-white border-slate-900 shadow-2xs font-semibold'
                  : 'bg-white text-slate-600 border-slate-200 hover:border-slate-300',
                isEditingProtocols ? 'cursor-pointer' : 'cursor-default opacity-90'
              ]"
              @click="toggleProtocol(proto.id)"
            >
              <span
                class="w-2 h-2 rounded-full"
                :class="activeProtocols.includes(proto.id) ? 'bg-amber-400' : 'bg-slate-300'"
              />
              {{ proto.label }}
            </button>
          </div>

          <!-- 专属 URL 输入字段折叠 (编辑态) / 概览行 (展示态) -->
          <div v-if="isEditingProtocols" class="pt-2 space-y-2.5">
            <div v-if="activeProtocols.includes('chat')">
              <label class="block text-xs font-medium text-slate-600">OpenAI Chat 专属 Base URL</label>
              <input
                v-model="customUrls.chat"
                type="url"
                placeholder="未单独覆盖时统一走 Base URL"
                class="w-full bg-slate-50 border border-slate-200 rounded-lg px-3 py-2 text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-400 focus:bg-white"
                data-testid="chat-url-input"
              />
            </div>
            <div v-if="activeProtocols.includes('messages')">
              <label class="block text-xs font-medium text-slate-600">Anthropic Messages 专属 Base URL</label>
              <input
                v-model="customUrls.messages"
                type="url"
                placeholder="未单独覆盖时统一走 Base URL"
                class="w-full bg-slate-50 border border-slate-200 rounded-lg px-3 py-2 text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-400 focus:bg-white"
                data-testid="messages-url-input"
              />
            </div>
            <div v-if="activeProtocols.includes('responses')">
              <label class="block text-xs font-medium text-slate-600">OpenAI Responses 专属 Base URL</label>
              <input
                v-model="customUrls.responses"
                type="url"
                placeholder="未单独覆盖时统一走 Base URL"
                class="w-full bg-slate-50 border border-slate-200 rounded-lg px-3 py-2 text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-400 focus:bg-white"
                data-testid="responses-url-input"
              />
            </div>
          </div>
          <div v-else-if="hasCustomUrls" class="pt-1.5 text-xs text-slate-500 space-y-1 font-mono">
            <div v-if="provider.chat_url" class="truncate" :title="provider.chat_url">Chat 端点: {{ provider.chat_url }}</div>
            <div v-if="provider.messages_url" class="truncate" :title="provider.messages_url">Messages 端点: {{ provider.messages_url }}</div>
            <div v-if="provider.responses_url" class="truncate" :title="provider.responses_url">Responses 端点: {{ provider.responses_url }}</div>
          </div>
        </div>

        <!-- 密钥子区域 (默认折叠) -->
        <div class="bg-white rounded-xl p-4.5 border border-slate-200/80 shadow-2xs">
          <KeySubSection
            :provider-name="provider.name"
            :keys="keys"
            :admin-write-enabled="adminWriteEnabled"
            :key-test-results="keyTestResults"
            :testing-key-ids="testingKeyIds"
            @create="(payload) => emit('create-key', payload)"
            @delete="(id) => emit('delete-key', id)"
            @test-single="(id) => emit('test-single-key', id)"
            @oauth-antigravity="(name) => emit('oauth-antigravity', name)"
          />
        </div>

        <!-- 模型子区域 (默认折叠) -->
        <div class="bg-white rounded-xl p-4.5 border border-slate-200/80 shadow-2xs">
          <ModelSubSection
            :provider-name="provider.name"
            :models="models"
            :on-batch-create="onBatchCreate"
            :admin-write-enabled="adminWriteEnabled"
            @create="(payload) => emit('create-model', payload)"
            @update="(name, payload) => emit('update-model', name, payload)"
            @delete="(name) => emit('delete-model', name)"
            @notice="(message) => emit('notice', message)"
          />
        </div>
      </div>
    </UiCollapsible>
  </div>
</template>
