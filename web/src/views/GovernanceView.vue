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
import type { CreateProviderPayload } from '../types/admin';

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
  editProvider,
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

type TabType = 'providers' | 'strategy';
const currentTab = ref<TabType>('providers');

// 行内新建服务商状态
const isAddingProvider = ref(false);
const providerSubmitting = ref(false);
const providerFormError = ref<string | null>(null);
const newProviderForm = ref<CreateProviderPayload>({
  name: '',
  base_url: 'https://tokens.ponyjob.top/v1',
  default_model: '',
  strategy: 'economy',
  billing_mode: 'token',
  input_price: 0,
  cached_price: 0,
  output_price: 0,
});

const newProviderProtocols = ref<string[]>(['chat']);
const newProviderCustomUrls = ref({
  chat: '',
  messages: '',
  responses: '',
});

function openAddProvider() {
  newProviderForm.value = {
    name: '',
    base_url: 'https://tokens.ponyjob.top/v1',
    default_model: '',
    strategy: 'economy',
    billing_mode: 'token',
    input_price: 0,
    cached_price: 0,
    output_price: 0,
  };
  newProviderProtocols.value = ['chat'];
  newProviderCustomUrls.value = { chat: '', messages: '', responses: '' };
  providerFormError.value = null;
  isAddingProvider.value = true;
}

function cancelAddProvider() {
  isAddingProvider.value = false;
  providerFormError.value = null;
}

async function handleSaveProvider() {
  const name = newProviderForm.value.name.trim();
  const url = newProviderForm.value.base_url.trim() || 'https://tokens.ponyjob.top/v1';
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
      default_protocol: newProviderProtocols.value[0] || 'chat',
      chat_url: newProviderProtocols.value.includes('chat') ? (newProviderCustomUrls.value.chat.trim() || null) : null,
      messages_url: newProviderProtocols.value.includes('messages') ? (newProviderCustomUrls.value.messages.trim() || null) : null,
      responses_url: newProviderProtocols.value.includes('responses') ? (newProviderCustomUrls.value.responses.trim() || null) : null,
    });
    isAddingProvider.value = false;
  } catch (err: unknown) {
    providerFormError.value = err instanceof Error ? err.message : String(err);
  } finally {
    providerSubmitting.value = false;
  }
}

async function handleRefresh() {
  clearConflict();
  await fetchAll().catch(() => {});
}
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
      <div class="flex flex-col sm:flex-row sm:items-center justify-between gap-4 mb-7">
        <div>
          <h1 class="text-2xl font-bold tracking-tight text-slate-900 flex items-center gap-2.5">
            模型管理
            <UiBadge variant="secondary" class="font-mono text-xs font-semibold" data-testid="config-version-val">
              v{{ configVersion }}
            </UiBadge>
          </h1>
          <p class="text-sm text-slate-500 mt-1.5">
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
              <Icons name="refresh" size="14" :class="loading ? 'animate-spin' : ''" />
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
              <Icons name="zap" size="14" class="text-amber-500" />
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
            <Icons name="plus" size="14" />
            服务商
          </UiButton>
        </div>
      </div>

      <!-- 拨测进度条 -->
      <div v-if="batchTesting.running" class="h-1.5 w-full bg-slate-200/80 rounded-full mb-6 overflow-hidden">
        <div
          class="h-full bg-indigo-600 transition-all duration-200"
          :style="{ width: `${(batchTesting.current / Math.max(1, batchTesting.total)) * 100}%` }"
        />
      </div>

      <!-- 错误提示横幅 -->
      <div v-if="error" class="flex items-center justify-between p-3.5 mb-6 bg-rose-50 border border-rose-200 rounded-xl text-xs text-rose-700">
        <span>配置获取失败: {{ error }}</span>
        <UiButton variant="ghost" size="sm" class="text-rose-700 hover:bg-rose-100/60" @click="handleRefresh">
          重试
        </UiButton>
      </div>

      <!-- 行内平滑展开：新建服务商表单 (替代原有抽屉) -->
      <UiCollapsible :open="isAddingProvider">
        <div class="borderless-card p-5 mb-6">
          <div class="flex items-center justify-between mb-3.5">
            <span class="font-bold text-base text-slate-800 flex items-center gap-2">
              <Icons name="server" size="18" class="text-indigo-600" />
              新建服务商
            </span>
            <button type="button" class="text-slate-400 hover:text-slate-600 cursor-pointer" @click="cancelAddProvider">
              <Icons name="cross" size="15" />
            </button>
          </div>

          <div v-if="providerFormError" class="p-2.5 mb-3.5 bg-rose-50 text-rose-600 rounded-lg text-xs font-medium">
            {{ providerFormError }}
          </div>

          <form class="space-y-3.5" @submit.prevent="handleSaveProvider">
            <div class="grid grid-cols-1 sm:grid-cols-3 gap-3">
              <div>
                <label class="block text-xs font-medium text-slate-600 mb-1">服务商标识 *</label>
                <input
                  v-model="newProviderForm.name"
                  type="text"
                  placeholder="例如: openai / deepseek"
                  required
                  class="w-full bg-slate-50 border border-slate-200/80 rounded-lg px-3 py-2 text-xs text-slate-900 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500 focus:bg-white"
                  data-testid="provider-name-input"
                />
              </div>

              <div class="sm:col-span-2">
                <label class="block text-xs font-medium text-slate-600 mb-1">Base URL *</label>
                <input
                  v-model="newProviderForm.base_url"
                  type="url"
                  placeholder="https://tokens.ponyjob.top/v1"
                  required
                  class="w-full bg-slate-50 border border-slate-200/80 rounded-lg px-3 py-2 text-xs text-slate-900 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500 focus:bg-white"
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
                  class="w-full bg-slate-50 border border-slate-200/80 rounded-lg px-3 py-2 text-xs text-slate-900 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500 focus:bg-white"
                  data-testid="provider-default-model-input"
                />
              </div>

              <div>
                <label class="block text-xs font-medium text-slate-600 mb-1">路由调度算法</label>
                <select
                  v-model="newProviderForm.strategy"
                  class="w-full bg-slate-50 border border-slate-200/80 rounded-lg px-3 py-2 text-xs text-slate-900 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500 focus:bg-white"
                >
                  <option value="round_robin">轮询 (Round Robin)</option>
                  <option value="priority">主备优先级 (Priority)</option>
                  <option value="weighted_round_robin">加权轮询 (Weighted)</option>
                  <option value="economy">经济优先 (Economy)</option>
                  <option value="speed">速度优先 (Speed)</option>
                  <option value="reliable">稳定优先 (Reliable)</option>
                  <option value="balanced">综合均衡 (Balanced)</option>
                </select>
              </div>
            </div>

            <!-- 支持模型协议选择器 (非下拉胶囊药丸) -->
            <div>
              <label class="block text-xs font-medium text-slate-600 mb-1.5">支持模型协议</label>
              <div class="flex flex-wrap gap-2">
                <button
                  v-for="proto in [
                    { id: 'chat', label: 'OpenAI Chat' },
                    { id: 'messages', label: 'Anthropic Messages' },
                    { id: 'responses', label: 'OpenAI Responses' },
                  ]"
                  :key="proto.id"
                  type="button"
                  class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium transition-all border select-none cursor-pointer"
                  :class="newProviderProtocols.includes(proto.id)
                    ? 'bg-amber-100 text-amber-900 border-amber-300 font-semibold shadow-2xs'
                    : 'bg-slate-50 text-slate-500 border-slate-200/80 hover:bg-slate-100/80'"
                  @click="
                    newProviderProtocols.includes(proto.id)
                      ? (newProviderProtocols.length > 1 && newProviderProtocols.splice(newProviderProtocols.indexOf(proto.id), 1))
                      : newProviderProtocols.push(proto.id)
                  "
                >
                  <span
                    class="w-2 h-2 rounded-full"
                    :class="newProviderProtocols.includes(proto.id) ? 'bg-amber-500' : 'bg-slate-300'"
                  />
                  {{ proto.label }}
                </button>
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
          class="px-3.5 py-1.5 text-xs font-medium rounded-lg transition-colors cursor-pointer"
          :class="currentTab === 'providers' ? 'bg-white shadow-2xs text-indigo-600 font-semibold' : 'text-slate-500 hover:text-slate-800'"
          data-testid="tab-providers"
          @click="currentTab = 'providers'"
        >
          全部服务商 ({{ providers.length }})
        </button>


        <button
          type="button"
          class="px-3.5 py-1.5 text-xs font-medium rounded-lg transition-colors cursor-pointer"
          :class="currentTab === 'strategy' ? 'bg-white shadow-2xs text-indigo-600 font-semibold' : 'text-slate-500 hover:text-slate-800'"
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
            :models="models.filter((m) => m.provider ? m.provider === p.name : true)"
            :keys="keys.filter((k) => k.provider === p.name)"
            :admin-write-enabled="adminWriteEnabled"
            :key-test-results="keyTestResults"
            :testing-key-ids="testingKeyIds"
            @delete-provider="removeProvider"
            @update-provider="editProvider"
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
