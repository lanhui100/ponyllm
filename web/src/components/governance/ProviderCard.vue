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
}>();

const emit = defineEmits<{
  (e: 'delete-provider', name: string): Promise<void>;
  (e: 'update-provider', name: string, payload: UpdateProviderPayload): Promise<void>;
  (e: 'create-model', payload: CreateModelPayload): Promise<void>;
  (e: 'update-model', name: string, payload: UpdateModelPayload): Promise<void>;
  (e: 'delete-model', name: string): Promise<void>;
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
    class="borderless-card mb-4 overflow-hidden"
    data-testid="provider-row"
  >
    <!-- 服务商一级卡片头部 (一等常显) -->
    <div
      class="p-4.5 flex items-center justify-between gap-4 cursor-pointer hover:bg-slate-50/50 transition-colors select-none"
      @click="expanded = !expanded"
    >
      <!-- 左侧：厂商标识与摘要 -->
      <div class="flex items-center gap-3.5 min-w-0">
        <!-- 暖橙色图标 -->
        <div class="w-10 h-10 rounded-xl bg-orange-50 text-orange-600 border border-orange-200/60 flex items-center justify-center shrink-0 font-bold text-sm">
          <Icons name="server" size="20" />
        </div>

        <div class="min-w-0">
          <div class="flex items-center gap-2.5">
            <span class="font-bold text-slate-900 text-base tracking-tight truncate">
              {{ provider.name }}
            </span>
            <UiBadge variant="secondary" class="text-xs font-medium">
              {{ formatStrategyLabel(provider.strategy) }}
            </UiBadge>
          </div>

          <!-- 去除敏感明文 URL，仅在有默认模型时显示默认模型 -->
          <div v-if="provider.default_model" class="flex items-center gap-2 text-xs text-slate-500 mt-1 truncate">
            <span>默认模型: {{ provider.default_model }}</span>
          </div>
        </div>
      </div>

      <!-- 右侧：概览徽标与一等纯图标操作组 -->
      <div class="flex items-center gap-2.5 shrink-0" @click.stop>
        <div class="hidden sm:flex items-center gap-2 mr-2">
          <UiBadge variant="default" class="text-xs font-medium">
            {{ models.length }} 模型
          </UiBadge>
          <UiBadge :variant="activeKeysCount > 0 ? 'success' : 'secondary'" class="text-xs font-medium">
            {{ activeKeysCount }}/{{ keys.length }} 密钥可用
          </UiBadge>
        </div>

        <!-- ⚡ 一键测速该服务商全部 Key -->
        <UiTooltip content="一键测试该服务商下所有密钥的连通性与延迟">
          <UiButton
            variant="ghost"
            size="icon"
            :disabled="keys.length === 0 || !adminWriteEnabled"
            class="text-amber-500 hover:text-amber-600 hover:bg-amber-50"
            @click="handleBatchTest"
          >
            <Icons name="zap" size="14" />
          </UiButton>
        </UiTooltip>

        <!-- 🗑 删除服务商 -->
        <UiTooltip content="删除该服务商">
          <UiButton
            variant="ghost"
            size="icon"
            :disabled="!adminWriteEnabled"
            data-testid="delete-provider-btn"
            class="text-slate-400 hover:text-rose-600 hover:bg-rose-50"
            @click="handleDeleteProvider"
          >
            <Icons name="trash" size="14" />
          </UiButton>
        </UiTooltip>

        <!-- 展开/折叠切换 -->
        <UiButton
          variant="ghost"
          size="icon"
          class="text-slate-400 hover:text-slate-700"
          @click="expanded = !expanded"
        >
          <Icons :name="expanded ? 'chevron-down' : 'chevron-right'" size="15" />
        </UiButton>
      </div>
    </div>

    <!-- 二级折叠展开区域 (包含模型协议、密钥及模型) -->
    <UiCollapsible :open="expanded">
      <div class="px-4.5 pb-4.5 pt-2 border-t border-slate-100 bg-slate-50/50 space-y-4">
        <!-- 模型协议选择器与专属端点 (同级非下拉多选) -->
        <div class="bg-white rounded-xl p-4 shadow-2xs space-y-3" data-testid="protocol-section">
          <div class="flex items-center justify-between">
            <div class="flex items-center gap-1.5 text-xs font-semibold text-slate-700">
              <Icons name="activity" size="14" class="text-amber-600" />
              模型协议
              <UiTooltip content="该服务商默认提供的协议与端点，未覆盖时统一走 Base URL">
                <Icons name="info" size="12" class="text-slate-400 cursor-pointer" />
              </UiTooltip>
            </div>

            <div v-if="adminWriteEnabled">
              <UiButton
                v-if="!isEditingProtocols"
                variant="ghost"
                size="sm"
                class="text-indigo-600 hover:text-indigo-700 hover:bg-indigo-50/60 text-xs py-1"
                data-testid="edit-protocols-btn"
                @click="isEditingProtocols = true"
              >
                配置端点
              </UiButton>
              <div v-else class="flex items-center gap-1.5">
                <UiButton
                  variant="ghost"
                  size="sm"
                  class="text-xs py-1"
                  @click="cancelEditProtocols"
                >
                  取消
                </UiButton>
                <UiButton
                  size="sm"
                  class="text-xs py-1"
                  :disabled="protocolsSaving"
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
              class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium transition-all border select-none cursor-pointer disabled:cursor-default"
              :class="activeProtocols.includes(proto.id)
                ? 'bg-amber-100 text-amber-900 border-amber-300 font-semibold shadow-2xs'
                : 'bg-slate-50 text-slate-500 border-slate-200/80 hover:bg-slate-100/80'"
              :data-testid="`protocol-pill-${proto.id}`"
              @click="toggleProtocol(proto.id)"
            >
              <span
                class="w-2 h-2 rounded-full"
                :class="activeProtocols.includes(proto.id) ? 'bg-amber-500' : 'bg-slate-300'"
              />
              {{ proto.label }}
            </button>
          </div>

          <!-- 各协议专属端点配置 (可编辑模式或已配置展示) -->
          <div v-if="isEditingProtocols" class="space-y-2 pt-2 border-t border-slate-100 text-xs">
            <div v-if="activeProtocols.includes('chat')" class="space-y-1">
              <label class="block text-3xs font-medium text-slate-500">OpenAI Chat 专属 Base URL</label>
              <input
                v-model="customUrls.chat"
                type="url"
                placeholder="未单独覆盖时统一走 Base URL"
                class="w-full bg-slate-50 border border-slate-200 rounded-lg px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-1 focus:ring-indigo-500"
                data-testid="chat-url-input"
              />
            </div>
            <div v-if="activeProtocols.includes('messages')" class="space-y-1">
              <label class="block text-3xs font-medium text-slate-500">Anthropic Messages 专属 Base URL</label>
              <input
                v-model="customUrls.messages"
                type="url"
                placeholder="未单独覆盖时统一走 Base URL"
                class="w-full bg-slate-50 border border-slate-200 rounded-lg px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-1 focus:ring-indigo-500"
                data-testid="messages-url-input"
              />
            </div>
            <div v-if="activeProtocols.includes('responses')" class="space-y-1">
              <label class="block text-3xs font-medium text-slate-500">OpenAI Responses 专属 Base URL</label>
              <input
                v-model="customUrls.responses"
                type="url"
                placeholder="未单独覆盖时统一走 Base URL"
                class="w-full bg-slate-50 border border-slate-200 rounded-lg px-2.5 py-1.5 text-xs text-slate-800 focus:outline-none focus:ring-1 focus:ring-indigo-500"
                data-testid="responses-url-input"
              />
            </div>
          </div>
          <div v-else-if="hasCustomUrls" class="pt-1 text-3xs text-slate-400 space-y-0.5 font-mono">
            <div v-if="provider.chat_url">Chat 端点: {{ provider.chat_url }}</div>
            <div v-if="provider.messages_url">Messages 端点: {{ provider.messages_url }}</div>
            <div v-if="provider.responses_url">Responses 端点: {{ provider.responses_url }}</div>
          </div>
        </div>

        <!-- 密钥子区域 (默认折叠) -->
        <div class="bg-white rounded-xl p-4 shadow-2xs">
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
        <div class="bg-white rounded-xl p-4 shadow-2xs">
          <ModelSubSection
            :provider-name="provider.name"
            :models="models"
            :admin-write-enabled="adminWriteEnabled"
            @create="(payload) => emit('create-model', payload)"
            @update="(name, payload) => emit('update-model', name, payload)"
            @delete="(name) => emit('delete-model', name)"
          />
        </div>
      </div>
    </UiCollapsible>
  </div>
</template>
