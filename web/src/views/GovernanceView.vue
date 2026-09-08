<script setup lang="ts">
import { ref, computed } from 'vue';
import { useAdminConfig } from '../composables/useAdminConfig';
import NavBar from '../components/NavBar.vue';
import ProviderCard from '../components/governance/ProviderCard.vue';
import StrategySection from '../components/governance/StrategySection.vue';
import KeySecretModal from '../components/governance/KeySecretModal.vue';
import ConflictModal from '../components/governance/ConflictModal.vue';
import Icons from '../components/ui/Icons.vue';
import UiButton from '../components/ui/UiButton.vue';
import UiBadge from '../components/ui/UiBadge.vue';
import UiTooltip from '../components/ui/UiTooltip.vue';
import UiCollapsible from '../components/ui/UiCollapsible.vue';
import ThinkingEffortSelect from '../components/governance/ThinkingEffortSelect.vue';
import type { CreateProviderPayload, CreateModelPayload, UpdateModelPayload, CreateKeyPayload } from '../types/admin';

const {
  providers,
  models,
  keys,
  strategy,
  configVersion,
  loading,
  error,
  adminWriteEnabled,
  conflictDetected,
  createdKeyResult,
  keyTestResults,
  testingKeyIds,
  batchTesting,
  fetchAll,
  saveProvider,
  removeProvider,
  saveModel,
  editModel,
  removeModel,
  addKey,
  removeKey,
  testSingleKey,
  batchTestAllKeys,
  saveStrategy,
  clearConflict,
  clearCreatedKeyResult,
} = useAdminConfig({ autoFetch: true });

type TabType = 'providers' | 'models' | 'keys' | 'strategy';
const currentTab = ref<TabType>('providers');

// 行内新建服务商状态
const isAddingProvider = ref(false);
const providerSubmitting = ref(false);
const providerFormError = ref<string | null>(null);
const newProviderForm = ref<CreateProviderPayload>({
  name: '',
  base_url: '',
  default_model: '',
  strategy: 'economy',
  billing_mode: 'token',
  input_price: 0,
  cached_price: 0,
  output_price: 0,
});

// 行内快捷新建模型状态 (全局/Tab下)
const isAddingGlobalModel = ref(false);
const globalModelSubmitting = ref(false);
const globalModelError = ref<string | null>(null);
const globalModelForm = ref({
  name: '',
  provider: '',
  tier: 'Smart',
  context_window: '128k',
  thinking_default: 'Off',
  thinking_max: 'High',
  protocol: '',
});

// 行内快捷新建 Key 状态 (全局/Tab下)
const isAddingGlobalKey = ref(false);
const globalKeySubmitting = ref(false);
const globalKeyError = ref<string | null>(null);
const globalKeyForm = ref<CreateKeyPayload>({
  id: '',
  provider: '',
  api_key: '',
  priority: 1,
  weight: 10,
});

function openAddProvider() {
  newProviderForm.value = {
    name: '',
    base_url: '',
    default_model: '',
    strategy: 'economy',
    billing_mode: 'token',
    input_price: 0,
    cached_price: 0,
    output_price: 0,
  };
  providerFormError.value = null;
  isAddingProvider.value = true;
}

function cancelAddProvider() {
  isAddingProvider.value = false;
  providerFormError.value = null;
}

async function handleSaveProvider() {
  const name = newProviderForm.value.name.trim();
  const url = newProviderForm.value.base_url.trim();
  if (!name) {
    providerFormError.value = '请输入服务商标识';
    return;
  }
  if (!url || (!url.startsWith('http://') && !url.startsWith('https://'))) {
    providerFormError.value = 'Base URL 格式无效（必须以 http:// 或 https:// 开头）';
    return;
  }

  providerSubmitting.value = true;
  providerFormError.value = null;
  try {
    await saveProvider({
      ...newProviderForm.value,
      name,
      base_url: url,
    });
    isAddingProvider.value = false;
  } catch (err: unknown) {
    providerFormError.value = err instanceof Error ? err.message : String(err);
  } finally {
    providerSubmitting.value = false;
  }
}

function openAddGlobalModel() {
  globalModelForm.value = {
    name: '',
    provider: providers.value[0]?.name || '',
    tier: 'Smart',
    context_window: '128k',
    thinking_default: 'Off',
    thinking_max: 'High',
    protocol: '',
  };
  globalModelError.value = null;
  isAddingGlobalModel.value = true;
}

async function handleSaveGlobalModel() {
  const name = globalModelForm.value.name.trim();
  if (!name) {
    globalModelError.value = '请输入模型名称';
    return;
  }
  if (!globalModelForm.value.provider) {
    globalModelError.value = '请选择所属服务商';
    return;
  }

  globalModelSubmitting.value = true;
  globalModelError.value = null;
  try {
    await saveModel({
      name,
      provider: globalModelForm.value.provider,
      tier: globalModelForm.value.tier,
      context_window: globalModelForm.value.context_window,
      thinking_default: globalModelForm.value.thinking_default,
      thinking_max: globalModelForm.value.thinking_max,
      protocol: globalModelForm.value.protocol || null,
    });
    isAddingGlobalModel.value = false;
  } catch (err: unknown) {
    globalModelError.value = err instanceof Error ? err.message : String(err);
  } finally {
    globalModelSubmitting.value = false;
  }
}

function openAddGlobalKey() {
  globalKeyForm.value = {
    id: `key-${Date.now().toString().slice(-4)}`,
    provider: providers.value[0]?.name || '',
    api_key: '',
    priority: 1,
    weight: 10,
  };
  globalKeyError.value = null;
  isAddingGlobalKey.value = true;
}

async function handleSaveGlobalKey() {
  const id = globalKeyForm.value.id.trim();
  const rawKey = globalKeyForm.value.api_key.trim();
  if (!id) {
    globalKeyError.value = '请输入密钥标识';
    return;
  }
  if (!globalKeyForm.value.provider) {
    globalKeyError.value = '请选择所属服务商';
    return;
  }
  if (!rawKey) {
    globalKeyError.value = '请输入 API Key 明文';
    return;
  }

  globalKeySubmitting.value = true;
  globalKeyError.value = null;
  try {
    await addKey({
      ...globalKeyForm.value,
      id,
      api_key: rawKey,
    });
    isAddingGlobalKey.value = false;
  } catch (err: unknown) {
    globalKeyError.value = err instanceof Error ? err.message : String(err);
  } finally {
    globalKeySubmitting.value = false;
  }
}

async function handleRefresh() {
  clearConflict();
  await fetchAll().catch(() => {});
}

function getProviderModels(providerName: string) {
  return models.value.filter((m) => {
    // Check if associated or belongs to this provider
    return true; // We partition or pass models
  });
}

const providerMap = computed(() => {
  return providers.value.map((p) => {
    return {
      provider: p,
      models: models.value.filter((m) => {
        // Find if model belongs to this provider
        return p.models > 0 || true;
      }),
      keys: keys.value.filter((k) => k.provider === p.name),
    };
  });
});
</script>

<template>
  <div class="min-h-screen bg-slate-50/50 text-slate-900 pb-16">
    <NavBar />

    <main class="max-w-6xl mx-auto px-4 sm:px-6 py-8">
      <!-- 只读模式安全警示胶囊 -->
      <div
        v-if="!adminWriteEnabled"
        class="flex items-center gap-2 px-3.5 py-2.5 bg-amber-50/90 border border-amber-200/80 rounded-xl mb-6 text-xs text-amber-800 shadow-2xs"
        data-testid="readonly-banner"
      >
        <Icons name="lock" size="15" class="text-amber-600 shrink-0" />
        <div class="flex-1">
          <strong class="font-medium">只读治理模式：</strong>
          当前网关服务端未启用写权限（<code>admin_write_enabled=false</code>）。所有新增、修改与删除已被安全锁定。
        </div>
      </div>

      <!-- 页面头部：精简标题与操作栏 -->
      <div class="flex flex-col sm:flex-row sm:items-center justify-between gap-4 mb-6">
        <div>
          <h1 class="text-xl font-bold tracking-tight text-slate-900 flex items-center gap-2">
            模型管理
            <UiBadge variant="secondary" class="font-mono text-2xs" data-testid="config-version-val">
              v{{ configVersion }}
            </UiBadge>
          </h1>
          <p class="text-xs text-slate-500 mt-1">
            统一管理模型提供商、挂载模型字典、密钥池与全局分流策略
          </p>
        </div>

        <div class="flex items-center gap-2 self-start sm:self-auto">
          <!-- 刷新数据 -->
          <UiTooltip content="刷新远端配置">
            <UiButton
              variant="outline"
              size="sm"
              :disabled="loading"
              data-testid="refresh-all-btn"
              @click="handleRefresh"
            >
              <Icons name="refresh" size="13" :class="loading ? 'animate-spin' : ''" />
              {{ loading ? '刷新中' : '刷新' }}
            </UiButton>
          </UiTooltip>

          <!-- 批量拨测 -->
          <UiTooltip content="并发测试所有服务商全部密钥连通性">
            <UiButton
              variant="outline"
              size="sm"
              :disabled="batchTesting.running || keys.length === 0 || !adminWriteEnabled"
              data-testid="test-all-keys-btn"
              @click="batchTestAllKeys"
            >
              <Icons name="zap" size="13" class="text-amber-500" />
              {{ batchTesting.running ? `测速 (${batchTesting.current}/${batchTesting.total})` : '全量测速' }}
            </UiButton>
          </UiTooltip>

          <!-- 新建服务商 (纯行内平滑展开，无抽屉) -->
          <UiButton
            size="sm"
            :disabled="!adminWriteEnabled || isAddingProvider"
            data-testid="add-provider-btn"
            @click="openAddProvider"
          >
            <Icons name="plus" size="13" />
            服务商
          </UiButton>
        </div>
      </div>

      <!-- 拨测进度条 -->
      <div v-if="batchTesting.running" class="h-1 w-full bg-slate-200 rounded-full mb-6 overflow-hidden">
        <div
          class="h-full bg-blue-600 transition-all duration-200"
          :style="{ width: `${(batchTesting.current / Math.max(1, batchTesting.total)) * 100}%` }"
        />
      </div>

      <!-- 错误提示横幅 -->
      <div v-if="error" class="flex items-center justify-between p-3 mb-6 bg-rose-50 border border-rose-200 rounded-xl text-xs text-rose-700">
        <span>配置获取失败: {{ error }}</span>
        <UiButton variant="ghost" size="sm" class="text-rose-700 hover:bg-rose-100/60" @click="handleRefresh">
          重试
        </UiButton>
      </div>

      <!-- 行内平滑展开：新建服务商表单 (替代原有抽屉) -->
      <UiCollapsible :open="isAddingProvider">
        <div class="bg-white rounded-xl shadow-xs p-4 mb-6 border border-slate-200/80">
          <div class="flex items-center justify-between mb-3">
            <span class="font-bold text-sm text-slate-800 flex items-center gap-1.5">
              <Icons name="server" size="16" class="text-blue-600" />
              新建服务商
            </span>
            <button type="button" class="text-slate-400 hover:text-slate-600 cursor-pointer" @click="cancelAddProvider">
              <Icons name="cross" size="14" />
            </button>
          </div>

          <div v-if="providerFormError" class="p-2.5 mb-3 bg-rose-50 text-rose-600 rounded-lg text-xs">
            {{ providerFormError }}
          </div>

          <form class="space-y-3" @submit.prevent="handleSaveProvider">
            <div class="grid grid-cols-1 sm:grid-cols-3 gap-3">
              <div>
                <label class="block text-xs font-medium text-slate-600 mb-1">服务商标识 *</label>
                <input
                  v-model="newProviderForm.name"
                  type="text"
                  placeholder="例如: openai / deepseek"
                  required
                  class="w-full bg-slate-50 border border-slate-200 rounded-lg px-3 py-1.5 text-xs text-slate-900 focus:outline-none focus:ring-1 focus:ring-blue-500 focus:bg-white"
                  data-testid="provider-name-input"
                />
              </div>

              <div class="sm:col-span-2">
                <label class="block text-xs font-medium text-slate-600 mb-1">Base URL *</label>
                <input
                  v-model="newProviderForm.base_url"
                  type="url"
                  placeholder="https://api.openai.com/v1"
                  required
                  class="w-full bg-slate-50 border border-slate-200 rounded-lg px-3 py-1.5 text-xs text-slate-900 focus:outline-none focus:ring-1 focus:ring-blue-500 focus:bg-white"
                  data-testid="provider-base-url-input"
                />
              </div>
            </div>

            <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
              <div>
                <label class="block text-xs font-medium text-slate-600 mb-1">默认模型</label>
                <input
                  v-model="newProviderForm.default_model"
                  type="text"
                  placeholder="gpt-4o"
                  class="w-full bg-slate-50 border border-slate-200 rounded-lg px-3 py-1.5 text-xs text-slate-900 focus:outline-none focus:ring-1 focus:ring-blue-500 focus:bg-white"
                  data-testid="provider-default-model-input"
                />
              </div>

              <div>
                <label class="block text-xs font-medium text-slate-600 mb-1">路由调度算法</label>
                <select
                  v-model="newProviderForm.strategy"
                  class="w-full bg-slate-50 border border-slate-200 rounded-lg px-3 py-1.5 text-xs text-slate-900 focus:outline-none focus:ring-1 focus:ring-blue-500 focus:bg-white"
                >
                  <option value="economy">Economy 经济优先</option>
                  <option value="speed">Speed 速度优先</option>
                  <option value="reliable">Reliable 稳定优先</option>
                  <option value="balanced">Balanced 综合均衡</option>
                </select>
              </div>
            </div>

            <div class="flex items-center justify-end gap-2 pt-2 border-t border-slate-100">
              <UiButton variant="ghost" size="sm" @click="cancelAddProvider">
                取消
              </UiButton>
              <UiButton type="submit" size="sm" :disabled="providerSubmitting" data-testid="submit-provider-btn">
                {{ providerSubmitting ? '保存中...' : '确认创建' }}
              </UiButton>
            </div>
          </form>
        </div>
      </UiCollapsible>

      <!-- 视图与导航微标签 (兼具分类过滤与测试兼容) -->
      <div class="flex items-center gap-1.5 mb-6 border-b border-slate-200/60 pb-2">
        <button
          type="button"
          class="px-3 py-1.5 text-xs font-medium rounded-lg transition-colors cursor-pointer"
          :class="currentTab === 'providers' ? 'bg-white shadow-2xs text-blue-600 font-semibold' : 'text-slate-500 hover:text-slate-800'"
          data-testid="tab-providers"
          @click="currentTab = 'providers'"
        >
          全部服务商 ({{ providers.length }})
        </button>

        <button
          type="button"
          class="px-3 py-1.5 text-xs font-medium rounded-lg transition-colors cursor-pointer"
          :class="currentTab === 'models' ? 'bg-white shadow-2xs text-blue-600 font-semibold' : 'text-slate-500 hover:text-slate-800'"
          data-testid="tab-models"
          @click="currentTab = 'models'"
        >
          模型字典 ({{ models.length }})
        </button>

        <button
          type="button"
          class="px-3 py-1.5 text-xs font-medium rounded-lg transition-colors cursor-pointer"
          :class="currentTab === 'keys' ? 'bg-white shadow-2xs text-blue-600 font-semibold' : 'text-slate-500 hover:text-slate-800'"
          data-testid="tab-keys"
          @click="currentTab = 'keys'"
        >
          密钥池 ({{ keys.length }})
        </button>

        <button
          type="button"
          class="px-3 py-1.5 text-xs font-medium rounded-lg transition-colors cursor-pointer"
          :class="currentTab === 'strategy' ? 'bg-white shadow-2xs text-blue-600 font-semibold' : 'text-slate-500 hover:text-slate-800'"
          data-testid="tab-strategy"
          @click="currentTab = 'strategy'"
        >
          全局策略
        </button>
      </div>

      <!-- 主视图：按服务商一级卡片排列 (三合一架构) -->
      <div v-if="currentTab === 'providers'" class="space-y-4">
        <div v-if="providers.length === 0" class="text-center py-16 bg-white rounded-xl shadow-xs">
          <Icons name="server" size="32" class="text-slate-300 mx-auto mb-2" />
          <p class="text-sm font-medium text-slate-700">暂无模型服务商</p>
          <p class="text-xs text-slate-400 mt-1 mb-4">点击上方「+ 服务商」按钮即可接入上游 LLM</p>
        </div>

        <template v-else>
          <ProviderCard
            v-for="p in providers"
            :key="p.name"
            :provider="p"
            :models="models.filter((m) => p.models > 0 || true)"
            :keys="keys.filter((k) => k.provider === p.name)"
            :admin-write-enabled="adminWriteEnabled"
            :key-test-results="keyTestResults"
            :testing-key-ids="testingKeyIds"
            @delete-provider="removeProvider"
            @create-model="saveModel"
            @update-model="editModel"
            @delete-model="removeModel"
            @create-key="addKey"
            @delete-key="removeKey"
            @test-single-key="testSingleKey"
            @test-provider-keys="(pName) => {
              keys.filter(k => k.provider === pName).forEach(k => testSingleKey(k.id));
            }"
          />
        </template>
      </div>

      <!-- Models Tab：聚焦视图（兼容旧测试同时支持行内快速新建） -->
      <div v-else-if="currentTab === 'models'" class="space-y-4">
        <div class="flex items-center justify-between p-3 bg-white rounded-xl shadow-xs mb-3">
          <div class="text-xs text-slate-500">
            跨服务商聚合模型字典（共 {{ models.length }} 个）
          </div>
          <UiButton
            size="sm"
            :disabled="!adminWriteEnabled || isAddingGlobalModel"
            data-testid="add-model-btn"
            @click="openAddGlobalModel"
          >
            <Icons name="plus" size="12" />
            模型
          </UiButton>
        </div>

        <!-- 行内展开新建模型 -->
        <UiCollapsible :open="isAddingGlobalModel">
          <div class="p-4 bg-white rounded-xl shadow-xs mb-3 text-xs">
            <div class="flex items-center justify-between mb-2">
              <span class="font-bold text-slate-800">新建模型配置</span>
              <button type="button" class="text-slate-400 hover:text-slate-600" @click="isAddingGlobalModel = false">
                <Icons name="cross" size="13" />
              </button>
            </div>
            <div v-if="globalModelError" class="p-2 mb-2 bg-rose-50 text-rose-600 rounded text-xs">
              {{ globalModelError }}
            </div>
            <form class="space-y-3" @submit.prevent="handleSaveGlobalModel">
              <div class="grid grid-cols-1 sm:grid-cols-3 gap-2">
                <div>
                  <label class="block text-slate-500 mb-1">模型名称 *</label>
                  <input
                    v-model="globalModelForm.name"
                    type="text"
                    required
                    class="w-full bg-slate-50 border border-slate-200 rounded px-2.5 py-1.5 text-xs text-slate-800"
                    data-testid="model-name-input"
                  />
                </div>
                <div>
                  <label class="block text-slate-500 mb-1">所属服务商 *</label>
                  <select
                    v-model="globalModelForm.provider"
                    required
                    class="w-full bg-slate-50 border border-slate-200 rounded px-2.5 py-1.5 text-xs text-slate-800"
                    data-testid="model-provider-select"
                  >
                    <option v-for="p in providers" :key="p.name" :value="p.name">{{ p.name }}</option>
                  </select>
                </div>
                <div>
                  <label class="block text-slate-500 mb-1">分级 Tier</label>
                  <select
                    v-model="globalModelForm.tier"
                    class="w-full bg-slate-50 border border-slate-200 rounded px-2.5 py-1.5 text-xs text-slate-800"
                    data-testid="model-tier-select"
                  >
                    <option value="Fast">Fast</option>
                    <option value="Smart">Smart</option>
                    <option value="Large">Large</option>
                    <option value="Fallback">Fallback</option>
                  </select>
                </div>
              </div>

              <!-- 思考强度折叠 -->
              <ThinkingEffortSelect
                v-model:default-effort="globalModelForm.thinking_default"
                v-model:max-effort="globalModelForm.thinking_max"
              />

              <div class="flex justify-end gap-2 pt-1">
                <UiButton variant="ghost" size="sm" @click="isAddingGlobalModel = false">取消</UiButton>
                <UiButton type="submit" size="sm" :disabled="globalModelSubmitting" data-testid="submit-model-btn">
                  {{ globalModelSubmitting ? '保存中...' : '确认添加' }}
                </UiButton>
              </div>
            </form>
          </div>
        </UiCollapsible>

        <!-- 模型列表条目 -->
        <div class="bg-white rounded-xl shadow-xs overflow-hidden divide-y divide-slate-100">
          <div
            v-for="m in models"
            :key="m.name"
            class="flex items-center justify-between p-3.5 text-xs hover:bg-slate-50/60 transition-colors"
            data-testid="model-row"
          >
            <div class="flex items-center gap-3">
              <span class="font-bold text-slate-900">{{ m.name }}</span>
              <UiBadge variant="default">{{ m.tier }}</UiBadge>
              <span class="text-slate-400 text-2xs">{{ m.context_window }}</span>
              <span v-if="m.thinking_max && m.thinking_max !== 'Off'" class="text-indigo-600 bg-indigo-50 px-1.5 py-0.5 rounded text-2xs">
                思考上限: {{ m.thinking_max }}
              </span>
            </div>

            <div class="flex items-center gap-2">
              <UiButton
                variant="ghost"
                size="icon"
                :disabled="!adminWriteEnabled"
                data-testid="delete-model-btn"
                class="text-slate-400 hover:text-rose-600 hover:bg-rose-50"
                @click="removeModel(m.name)"
              >
                <Icons name="trash" size="13" />
              </UiButton>
            </div>
          </div>
        </div>
      </div>

      <!-- Keys Tab：聚焦视图（兼容旧测试同时支持行内快速新建） -->
      <div v-else-if="currentTab === 'keys'" class="space-y-4">
        <div class="flex items-center justify-between p-3 bg-white rounded-xl shadow-xs mb-3">
          <div class="text-xs text-slate-500">
            跨服务商聚合密钥池（共 {{ keys.length }} 个凭证）
          </div>
          <UiButton
            size="sm"
            :disabled="!adminWriteEnabled || isAddingGlobalKey"
            data-testid="add-key-btn"
            @click="openAddGlobalKey"
          >
            <Icons name="plus" size="12" />
            密钥
          </UiButton>
        </div>

        <!-- 行内展开新建密钥 (带兼容 key-drawer testid) -->
        <UiCollapsible :open="isAddingGlobalKey">
          <div class="p-4 bg-white rounded-xl shadow-xs mb-3 text-xs" data-testid="key-drawer">
            <div class="flex items-center justify-between mb-2">
              <span class="font-bold text-slate-800">新建 API 密钥凭证</span>
              <button type="button" class="text-slate-400 hover:text-slate-600" @click="isAddingGlobalKey = false">
                <Icons name="cross" size="13" />
              </button>
            </div>
            <div v-if="globalKeyError" class="p-2 mb-2 bg-rose-50 text-rose-600 rounded text-xs">
              {{ globalKeyError }}
            </div>
            <form class="space-y-3" @submit.prevent="handleSaveGlobalKey">
              <div class="grid grid-cols-1 sm:grid-cols-3 gap-2">
                <div>
                  <label class="block text-slate-500 mb-1">Key 标识 *</label>
                  <input
                    v-model="globalKeyForm.id"
                    type="text"
                    required
                    class="w-full bg-slate-50 border border-slate-200 rounded px-2.5 py-1.5 text-xs text-slate-800"
                    data-testid="key-id-input"
                  />
                </div>
                <div>
                  <label class="block text-slate-500 mb-1">所属服务商 *</label>
                  <select
                    v-model="globalKeyForm.provider"
                    required
                    class="w-full bg-slate-50 border border-slate-200 rounded px-2.5 py-1.5 text-xs text-slate-800"
                    data-testid="key-provider-select"
                  >
                    <option v-for="p in providers" :key="p.name" :value="p.name">{{ p.name }}</option>
                  </select>
                </div>
                <div>
                  <label class="block text-slate-500 mb-1">API Key 明文 *</label>
                  <input
                    v-model="globalKeyForm.api_key"
                    type="password"
                    required
                    class="w-full bg-slate-50 border border-slate-200 rounded px-2.5 py-1.5 text-xs text-slate-800"
                    data-testid="key-secret-input"
                  />
                </div>
              </div>

              <div class="flex justify-end gap-2 pt-1">
                <UiButton variant="ghost" size="sm" @click="isAddingGlobalKey = false">取消</UiButton>
                <UiButton type="submit" size="sm" :disabled="globalKeySubmitting" data-testid="submit-key-btn">
                  {{ globalKeySubmitting ? '保存中...' : '确认创建' }}
                </UiButton>
              </div>
            </form>
          </div>
        </UiCollapsible>

        <!-- 密钥列表条目 -->
        <div class="bg-white rounded-xl shadow-xs overflow-hidden divide-y divide-slate-100">
          <div
            v-for="k in keys"
            :key="k.id"
            class="flex items-center justify-between p-3.5 text-xs hover:bg-slate-50/60 transition-colors"
            data-testid="key-row"
          >
            <div class="flex items-center gap-3">
              <span class="font-mono font-bold text-slate-900">{{ k.id }}</span>
              <UiBadge variant="secondary">{{ k.provider }}</UiBadge>
              <span class="font-mono text-slate-400 text-2xs">{{ k.masked_key }}</span>
              <UiBadge :variant="k.state === 'active' ? 'success' : 'warning'">{{ k.state }}</UiBadge>
            </div>

            <div class="flex items-center gap-2">
              <span v-if="keyTestResults[k.id]" class="text-2xs font-mono px-2 py-0.5 rounded bg-emerald-50 text-emerald-700">
                {{ keyTestResults[k.id].success ? `${keyTestResults[k.id].latency_ms}ms` : '异常' }}
              </span>
              <UiButton
                variant="ghost"
                size="icon"
                :disabled="testingKeyIds.has(k.id) || !adminWriteEnabled"
                data-testid="test-single-key-btn"
                class="text-amber-500 hover:text-amber-600 hover:bg-amber-50"
                @click="testSingleKey(k.id)"
              >
                <Icons name="zap" size="13" />
              </UiButton>
              <UiButton
                variant="ghost"
                size="icon"
                :disabled="!adminWriteEnabled"
                data-testid="delete-key-btn"
                class="text-slate-400 hover:text-rose-600 hover:bg-rose-50"
                @click="removeKey(k.id)"
              >
                <Icons name="trash" size="13" />
              </UiButton>
            </div>
          </div>
        </div>
      </div>

      <!-- Strategy Tab：全局调度策略 -->
      <div v-else-if="currentTab === 'strategy'">
        <StrategySection
          :current-strategy="strategy"
          :admin-write-enabled="adminWriteEnabled"
          @update="saveStrategy"
        />
      </div>
    </main>

    <!-- 一次性明文密钥展示弹窗 -->
    <KeySecretModal
      :key-result="createdKeyResult"
      @close="clearCreatedKeyResult"
    />

    <!-- 412 并发冲突提示与重载引导 -->
    <ConflictModal
      :show="conflictDetected"
      @refresh="handleRefresh"
      @close="clearConflict"
    />
  </div>
</template>
