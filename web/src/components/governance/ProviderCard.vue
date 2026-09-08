<script setup lang="ts">
import { ref, computed } from 'vue';
import type {
  ProviderView,
  ModelView,
  KeyView,
  KeyTestView,
  CreateModelPayload,
  UpdateModelPayload,
  CreateKeyPayload,
} from '../../types/admin';
import Icons from '../ui/Icons.vue';
import UiButton from '../ui/UiButton.vue';
import UiBadge from '../ui/UiBadge.vue';
import UiTooltip from '../ui/UiTooltip.vue';
import UiCollapsible from '../ui/UiCollapsible.vue';
import KeySubSection from './KeySubSection.vue';
import ModelSubSection from './ModelSubSection.vue';

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
  (e: 'create-model', payload: CreateModelPayload): Promise<void>;
  (e: 'update-model', name: string, payload: UpdateModelPayload): Promise<void>;
  (e: 'delete-model', name: string): Promise<void>;
  (e: 'create-key', payload: CreateKeyPayload): Promise<void>;
  (e: 'delete-key', id: string): Promise<void>;
  (e: 'test-single-key', id: string): Promise<void>;
  (e: 'test-provider-keys', providerName: string): Promise<void>;
}>();

const expanded = ref(props.defaultExpanded ?? true);
const isEditingProvider = ref(false);
const showAdvancedConfig = ref(false);

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
    class="bg-white rounded-xl shadow-xs transition-all duration-200 overflow-hidden mb-4"
    data-testid="provider-row"
  >
    <!-- 服务商一级卡片头部 (一等常显) -->
    <div
      class="p-4 flex items-center justify-between gap-4 cursor-pointer hover:bg-slate-50/50 transition-colors select-none"
      @click="expanded = !expanded"
    >
      <!-- 左侧：厂商标识与摘要 -->
      <div class="flex items-center gap-3 min-w-0">
        <div class="w-9 h-9 rounded-lg bg-blue-50 text-blue-600 flex items-center justify-center shrink-0 font-bold text-sm">
          <Icons name="server" size="18" />
        </div>

        <div class="min-w-0">
          <div class="flex items-center gap-2">
            <span class="font-bold text-slate-900 text-sm tracking-tight truncate">
              {{ provider.name }}
            </span>
            <UiBadge variant="secondary" class="text-2xs font-mono">
              {{ provider.strategy }}
            </UiBadge>
          </div>

          <div class="flex items-center gap-2 text-xs text-slate-400 mt-0.5 font-mono truncate">
            <span>{{ provider.base_url }}</span>
            <span v-if="provider.default_model" class="text-slate-500 font-sans">
              · 默认: {{ provider.default_model }}
            </span>
          </div>
        </div>
      </div>

      <!-- 右侧：概览徽标与一等纯图标操作组 -->
      <div class="flex items-center gap-2 shrink-0" @click.stop>
        <div class="hidden sm:flex items-center gap-1.5 mr-2">
          <UiBadge variant="default" class="text-2xs">
            {{ models.length }} 模型
          </UiBadge>
          <UiBadge :variant="activeKeysCount > 0 ? 'success' : 'secondary'" class="text-2xs">
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

    <!-- 二级折叠展开区域 (包含密钥、模型及高级配置) -->
    <UiCollapsible :open="expanded">
      <div class="px-4 pb-4 pt-1 border-t border-slate-100 bg-slate-50/30 space-y-4">
        <!-- 密钥凭证子区域 -->
        <div class="bg-white rounded-lg p-3 shadow-2xs">
          <KeySubSection
            :provider-name="provider.name"
            :keys="keys"
            :admin-write-enabled="adminWriteEnabled"
            :key-test-results="keyTestResults"
            :testing-key-ids="testingKeyIds"
            @create="(payload) => emit('create-key', payload)"
            @delete="(id) => emit('delete-key', id)"
            @test-single="(id) => emit('test-single-key', id)"
          />
        </div>

        <!-- 挂载模型子区域 -->
        <div class="bg-white rounded-lg p-3 shadow-2xs">
          <ModelSubSection
            :provider-name="provider.name"
            :models="models"
            :admin-write-enabled="adminWriteEnabled"
            @create="(payload) => emit('create-model', payload)"
            @update="(name, payload) => emit('update-model', name, payload)"
            @delete="(name) => emit('delete-model', name)"
          />
        </div>

        <!-- 服务商高级配置 (计费单价与详细参数折叠) -->
        <div class="pt-1">
          <button
            type="button"
            class="text-2xs text-slate-400 hover:text-slate-600 inline-flex items-center gap-1 cursor-pointer py-1 font-medium"
            @click="showAdvancedConfig = !showAdvancedConfig"
          >
            <Icons :name="showAdvancedConfig ? 'chevron-down' : 'chevron-right'" size="11" />
            计费单价与高级参数
          </button>

          <UiCollapsible :open="showAdvancedConfig">
            <div class="mt-2 p-3 bg-white rounded-lg shadow-2xs text-xs space-y-2">
              <div class="grid grid-cols-3 gap-2">
                <div class="p-2 bg-slate-50 rounded">
                  <span class="block text-2xs text-slate-400">输入单价 ($/M)</span>
                  <span class="font-mono text-slate-700 font-medium">{{ provider.input_price }}</span>
                </div>
                <div class="p-2 bg-slate-50 rounded">
                  <span class="block text-2xs text-slate-400">缓存命中单价 ($/M)</span>
                  <span class="font-mono text-slate-700 font-medium">{{ provider.cached_price }}</span>
                </div>
                <div class="p-2 bg-slate-50 rounded">
                  <span class="block text-2xs text-slate-400">输出单价 ($/M)</span>
                  <span class="font-mono text-slate-700 font-medium">{{ provider.output_price }}</span>
                </div>
              </div>
            </div>
          </UiCollapsible>
        </div>
      </div>
    </UiCollapsible>
  </div>
</template>
