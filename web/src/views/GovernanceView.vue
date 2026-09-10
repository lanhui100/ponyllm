<script setup lang="ts">
import { ref, onUnmounted } from 'vue';
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
import { resolveBaseURL } from '../lib/alova';
import type { CreateProviderPayload } from '../types/admin';
import { toast } from '../composables/useToast';

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
  proxyStatus,
  batchTesting,
  fetchAll,
  fetchProxyStatus,
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
  getAntigravityAuthUrl,
  getAntigravityPending,
  authorizeAntigravity,
} = useAdminConfig({ autoFetch: true });

type TabType = 'providers' | 'strategy';
const currentTab = ref<TabType>('providers');

/** 轻量 toast（单条，自动消失，接入全局中央毛玻璃 Toast）。 */
function showToast(message: string, ms = 3500) {
  toast.info(message, { duration: ms });
}

/** 上游名单批量添加：逐个复用 saveModel（版本控制内聚），409 记为跳过。 */
async function batchCreateModels(
  provider: string,
  ids: string[],
  onProgress: (done: number, total: number) => void,
): Promise<{ added: number; skipped: number; failed: number }> {
  let added = 0;
  let skipped = 0;
  let failed = 0;
  for (let i = 0; i < ids.length; i += 1) {
    try {
      await saveModel({
        name: ids[i],
        provider,
        tier: 'Smart',
        context_window: '256k',
        thinking_default: 'Off',
        thinking_max: 'High',
        input_types: ['text', 'image'],
        output_types: ['text'],
      });
      added += 1;
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      if (/409|already exists|已存在/.test(msg)) {
        skipped += 1;
      } else {
        failed += 1;
      }
    }
    onProgress(i + 1, ids.length);
  }
  return { added, skipped, failed };
}

type ProviderMode = 'standard' | 'antigravity';
const newProviderMode = ref<ProviderMode>('standard');
const showAgAdvanced = ref(false);

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

// Antigravity OAuth 专属表单状态
const antigravityForm = ref({
  provider: 'antigravity',
  id: '',
  code_or_url: '',
  priority: 1,
  weight: 10,
});
const antigravityAuthUrl = ref('');
const fetchingAuthUrl = ref(false);
const authUrlCopied = ref(false);
const probingProxy = ref(false);
const copiedPproxyOn = ref(false);
const oauthWaiting = ref(false);
const oauthState = ref<string | null>(null);
const oauthPopupRef = ref<Window | null>(null);
let pollTimer: ReturnType<typeof setInterval> | null = null;

async function probeProxy() {
  probingProxy.value = true;
  try {
    await fetchProxyStatus();
  } finally {
    probingProxy.value = false;
  }
}

async function copyPproxyOn() {
  if (typeof navigator !== 'undefined' && navigator.clipboard) {
    await navigator.clipboard.writeText('pproxy on');
    copiedPproxyOn.value = true;
    setTimeout(() => {
      copiedPproxyOn.value = false;
    }, 2000);
  }
}

function getEffectiveOAuthCallbackUri(): string | undefined {
  if (typeof window === 'undefined') return undefined;
  const baseUrl = resolveBaseURL();
  if (baseUrl) {
    return `${baseUrl}/oauth2callback`;
  }
  return `${window.location.origin}/oauth2callback`;
}

async function checkClipboardForOAuthCode() {
  if (!oauthWaiting.value) return;
  if (antigravityForm.value.code_or_url.trim()) return;
  try {
    if (typeof navigator !== 'undefined' && navigator.clipboard && typeof navigator.clipboard.readText === 'function') {
      const clipText = (await navigator.clipboard.readText()).trim();
      if (!clipText) return;
      // If clipboard contains code parameter or oauth2callback url
      if (clipText.includes('oauth2callback') || clipText.includes('code=') || (clipText.startsWith('4/') && clipText.length > 20)) {
        antigravityForm.value.code_or_url = clipText;
        showToast('已自动从剪贴板捕获授权凭据，正在换票...');
        void handleAuthorizeAntigravity();
      }
    }
  } catch {
    // Clipboard permission denied or unavailable - ignore silently
  }
}

function handleWindowFocus() {
  void checkClipboardForOAuthCode();
}

function cleanupOAuthSession() {
  oauthWaiting.value = false;
  oauthState.value = null;
  oauthPopupRef.value = null;
  if (pollTimer) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
  if (typeof window !== 'undefined') {
    window.removeEventListener('message', handleWindowMessage);
    window.removeEventListener('focus', handleWindowFocus);
  }
}

function handleWindowMessage(event: MessageEvent) {
  if (!event.data || event.data.type !== 'antigravity:oauth_callback') {
    return;
  }
  // 安全校验 1: 严格校验消息来源 Origin (允许同源，或后端配置的 target_origin)
  const expectedOrigin = getEffectiveOAuthCallbackUri() ? new URL(getEffectiveOAuthCallbackUri()!).origin : window.location.origin;
  if (typeof window !== 'undefined' && event.origin !== window.location.origin && event.origin !== expectedOrigin) {
    console.warn('[PonyLLM OAuth] 拒绝未授信跨源消息:', event.origin);
    return;
  }
  // 安全校验 2: 校验消息发送源 Window 引用 (若有)
  if (oauthPopupRef.value && event.source && event.source !== oauthPopupRef.value) {
    console.warn('[PonyLLM OAuth] 拒绝来自未知弹窗窗口的消息');
    return;
  }
  // 安全校验 3: 严格比对 State 防范 CSRF
  if (oauthState.value && event.data.state && event.data.state !== oauthState.value) {
    console.warn('[PonyLLM OAuth] 拒绝 State 不匹配的 OAuth 回调');
    return;
  }

  if (event.data.success && event.data.code) {
    antigravityForm.value.code_or_url = event.data.code;
    showToast('已成功接收 Google 授权回调，正在自动保存...');
    void handleAuthorizeAntigravity();
  } else if (event.data.error) {
    providerFormError.value = `Google 授权失败: ${event.data.error}`;
    cleanupOAuthSession();
  }
}

async function loadAntigravityAuthUrl() {
  fetchingAuthUrl.value = true;
  try {
    const originUri = getEffectiveOAuthCallbackUri();
    const res = await getAntigravityAuthUrl(originUri);
    antigravityAuthUrl.value = res.auth_url;
    oauthState.value = res.state;
  } catch (err: unknown) {
    providerFormError.value = `获取授权链接失败: ${err instanceof Error ? err.message : String(err)}`;
  } finally {
    fetchingAuthUrl.value = false;
  }
}

async function switchToAntigravityMode() {
  newProviderMode.value = 'antigravity';
  providerFormError.value = null;
  if (!antigravityAuthUrl.value) {
    await loadAntigravityAuthUrl();
  }
  void fetchProxyStatus().catch(() => {});
}

async function fetchAndOpenAuthUrl() {
  fetchingAuthUrl.value = true;
  providerFormError.value = null;
  try {
    const originUri = getEffectiveOAuthCallbackUri();
    const res = await getAntigravityAuthUrl(originUri);
    antigravityAuthUrl.value = res.auth_url;
    oauthState.value = res.state;

    if (typeof window !== 'undefined') {
      oauthWaiting.value = true;
      window.addEventListener('message', handleWindowMessage);
      window.addEventListener('focus', handleWindowFocus);

      const width = 600;
      const height = 720;
      const left = window.screenX + (window.outerWidth - width) / 2;
      const top = window.screenY + (window.outerHeight - height) / 2;
      oauthPopupRef.value = window.open(
        res.auth_url,
        'google_oauth_popup',
        `width=${width},height=${height},left=${left},top=${top},status=no,toolbar=no,menubar=no,noopener=no`
      );


      // 双重保障：开启轻量轮询 (最长 45s)
      let attempts = 0;
      if (pollTimer) clearInterval(pollTimer);
      pollTimer = setInterval(async () => {
        attempts++;
        if (attempts > 45 || !oauthWaiting.value) {
          cleanupOAuthSession();
          return;
        }
        try {
          if (oauthState.value) {
            const pending = await getAntigravityPending(oauthState.value);
            if (pending.ready) {
              if (pending.code) {
                antigravityForm.value.code_or_url = pending.code;
                showToast('已检测到授权完成，正在自动换票...');
                void handleAuthorizeAntigravity();
              } else if (pending.error) {
                providerFormError.value = `Google 授权失败: ${pending.error}`;
                cleanupOAuthSession();
              }
            }
          }
        } catch {
          // ignore network hiccups while polling
        }
      }, 1000);
    }
  } catch (err: unknown) {
    providerFormError.value = `获取授权链接失败: ${err instanceof Error ? err.message : String(err)}`;
  } finally {
    fetchingAuthUrl.value = false;
  }
}

async function copyAuthUrl() {
  if (!antigravityAuthUrl.value) return;
  try {
    if (typeof navigator !== 'undefined' && navigator.clipboard) {
      await navigator.clipboard.writeText(antigravityAuthUrl.value);
      authUrlCopied.value = true;
      setTimeout(() => {
        authUrlCopied.value = false;
      }, 2000);
    }
  } catch {
    // ignore
  }
}

function openAddProvider() {
  newProviderMode.value = 'standard';
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
  antigravityForm.value = {
    provider: 'antigravity',
    id: '',
    code_or_url: '',
    priority: 1,
    weight: 10,
  };
  antigravityAuthUrl.value = '';
  authUrlCopied.value = false;
  showAgAdvanced.value = false;
  providerFormError.value = null;
  cleanupOAuthSession();
  isAddingProvider.value = true;
}

function cancelAddProvider() {
  cleanupOAuthSession();
  isAddingProvider.value = false;
  providerFormError.value = null;
}

async function handleSaveProvider() {
  if (!adminWriteEnabled.value) return;
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

async function handleAuthorizeAntigravity() {
  if (!adminWriteEnabled.value) return;
  const codeOrUrl = antigravityForm.value.code_or_url.trim();
  if (!codeOrUrl) {
    providerFormError.value = '请输入重定向 URL 或 Code';
    return;
  }

  providerSubmitting.value = true;
  providerFormError.value = null;
  try {
    const originUri = getEffectiveOAuthCallbackUri();
    await authorizeAntigravity({
      code_or_url: codeOrUrl,
      provider: antigravityForm.value.provider.trim() || 'antigravity',
      id: antigravityForm.value.id.trim() || undefined,
      priority: antigravityForm.value.priority,
      weight: antigravityForm.value.weight,
      redirect_uri: originUri,
      state: oauthState.value || undefined,
    });
    cleanupOAuthSession();
    isAddingProvider.value = false;
    showToast(`Google Antigravity 账号已成功接入`);
  } catch (err: unknown) {
    providerFormError.value = err instanceof Error ? err.message : String(err);
  } finally {
    providerSubmitting.value = false;
  }
}

async function handleAuthorizeAntigravityKey(payload: {
  code_or_url: string;
  provider?: string | null;
  id?: string | null;
  priority?: number;
  weight?: number;
}) {
  if (!adminWriteEnabled.value) return;
  await authorizeAntigravity(payload);
}

async function handleOpenAntigravityForProvider(providerName: string) {
  openAddProvider();
  await switchToAntigravityMode();
  antigravityForm.value.provider = providerName;
}

async function handleRefresh() {
  clearConflict();
  await fetchAll().catch(() => {});
}

onUnmounted(() => {
  cleanupOAuthSession();
});
</script>

<template>
  <div class="min-h-screen bg-transparent text-slate-900 pb-16">
    <NavBar />

    <main class="max-w-6xl mx-auto px-4 sm:px-6 py-8">
      <!-- 只读模式安全警示胶囊 -->
      <div
        v-if="!adminWriteEnabled"
        class="flex items-center gap-2.5 px-4 py-3 bg-amber-50 border border-amber-200 rounded-xl mb-6 text-sm text-amber-800 shadow-2xs"
        data-testid="readonly-banner"
      >
        <Icons name="lock" size="16" class="text-amber-600 shrink-0" />
        <div class="flex-1">
          <strong class="font-semibold">只读治理模式：</strong>
          当前网关服务端未启用写权限（<code>admin_write_enabled=false</code>）。所有新增、修改与删除已被安全锁定。
        </div>
      </div>

      <!-- 页面头部：精简标题与操作栏 -->
      <div class="flex flex-col sm:flex-row sm:items-center justify-between gap-4 mb-7">
        <div>
          <h1 class="text-2xl font-bold tracking-tight text-slate-900 flex items-center gap-2.5">
            模型管理
            <UiBadge variant="secondary" class="font-mono text-[13px] font-semibold" data-testid="config-version-val">
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
      <div v-if="error" class="flex items-center justify-between p-4 mb-6 bg-rose-50 border border-rose-200 rounded-xl text-sm text-rose-700">
        <span>配置获取失败: {{ error }}</span>
        <UiButton variant="ghost" size="sm" class="text-rose-700 hover:bg-rose-100/60 text-[13px]" @click="handleRefresh">
          重试
        </UiButton>
      </div>

      <!-- 行内平滑展开：新建服务商表单 (替代原有抽屉) -->
      <UiCollapsible :open="isAddingProvider">
        <div class="swiss-card p-6 mb-6">
          <div class="flex items-center justify-between mb-4">
            <span class="font-bold text-lg text-slate-900 flex items-center gap-2">
              <Icons name="server" size="20" class="text-slate-800" />
              新建服务商
            </span>
            <button type="button" class="text-slate-400 hover:text-slate-700 cursor-pointer" @click="cancelAddProvider">
              <Icons name="cross" size="16" />
            </button>
          </div>

          <div v-if="providerFormError" class="p-3 mb-4 bg-rose-50 border border-rose-200 text-rose-700 rounded-lg text-sm font-medium">
            {{ providerFormError }}
          </div>

          <!-- 服务商类型切换 -->
          <div class="flex items-center gap-2 p-1 bg-white/40 rounded-lg w-fit mb-5 border border-white/50 backdrop-blur-xs">
            <button
              type="button"
              class="px-3.5 py-1.5 text-[13px] font-medium rounded-md transition-all cursor-pointer"
              :class="newProviderMode === 'standard' ? 'bg-white/80 text-slate-950 shadow-2xs font-semibold' : 'text-slate-600 hover:text-slate-900'"
              data-testid="mode-standard-btn"
              @click="newProviderMode = 'standard'"
            >
              通用服务商 (OpenAI / Claude 兼容)
            </button>
            <button
              type="button"
              class="inline-flex items-center gap-1.5 px-3.5 py-1.5 text-[13px] font-medium rounded-md transition-all cursor-pointer"
              :class="newProviderMode === 'antigravity' ? 'bg-white/80 text-slate-950 shadow-2xs font-semibold' : 'text-slate-600 hover:text-slate-900'"
              data-testid="mode-antigravity-btn"
              @click="switchToAntigravityMode"
            >
              <Icons name="zap" size="14" class="text-amber-500" />
              Google Antigravity (内置协议与 OAuth2 授权)
            </button>
          </div>

          <!-- 通用服务商表单 -->
          <form v-if="newProviderMode === 'standard'" class="space-y-4" @submit.prevent="handleSaveProvider">
            <div class="grid grid-cols-1 sm:grid-cols-3 gap-3.5">
              <div>
                <label class="block text-[13px] font-medium text-slate-700 mb-1.5">服务商标识 *</label>
                <input
                  v-model="newProviderForm.name"
                  type="text"
                  placeholder="例如: openai / deepseek"
                  required
                  class="w-full bg-white/60 border border-white/50 rounded-lg px-3.5 py-2 text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-400 focus:bg-white/90"
                  data-testid="provider-name-input"
                />
              </div>

              <div class="sm:col-span-2">
                <label class="block text-[13px] font-medium text-slate-700 mb-1.5">Base URL *</label>
                <input
                  v-model="newProviderForm.base_url"
                  type="url"
                  placeholder="https://tokens.ponyjob.top/v1"
                  required
                  class="w-full bg-white/60 border border-white/50 rounded-lg px-3.5 py-2 text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-400 focus:bg-white/90"
                  data-testid="provider-base-url-input"
                />
              </div>
            </div>

            <div class="grid grid-cols-1 sm:grid-cols-2 gap-3.5">
              <div>
                <label class="block text-[13px] font-medium text-slate-700 mb-1.5">默认模型</label>
                <input
                  v-model="newProviderForm.default_model"
                  type="text"
                  placeholder="gpt-4o"
                  class="w-full bg-white/60 border border-white/50 rounded-lg px-3.5 py-2 text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-400 focus:bg-white/90"
                  data-testid="provider-default-model-input"
                />
              </div>

              <div>
                <label class="block text-[13px] font-medium text-slate-700 mb-1.5">路由调度算法</label>
                <select
                  v-model="newProviderForm.strategy"
                  class="w-full bg-white/60 border border-white/50 rounded-lg px-3.5 py-2 text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-400 focus:bg-white/90"
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
              <label class="block text-[13px] font-medium text-slate-700 mb-2">支持模型协议</label>
              <div class="flex flex-wrap gap-2">
                <button
                  v-for="proto in [
                    { id: 'chat', label: 'OpenAI Chat' },
                    { id: 'messages', label: 'Anthropic Messages' },
                    { id: 'responses', label: 'OpenAI Responses' },
                  ]"
                  :key="proto.id"
                  type="button"
                  class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-[13px] font-medium transition-all border select-none cursor-pointer"
                  :class="newProviderProtocols.includes(proto.id)
                    ? 'bg-amber-100/90 text-amber-900 border-amber-300 font-semibold shadow-2xs'
                    : 'bg-white/60 text-slate-600 border-white/50 hover:bg-white/80'"
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

            <div class="flex items-center justify-end gap-2 pt-3 border-t border-slate-100">
              <UiButton variant="ghost" size="sm" @click="cancelAddProvider">
                取消
              </UiButton>
              <UiButton
                type="submit"
                size="sm"
                :disabled="providerSubmitting || !adminWriteEnabled"
                data-testid="submit-provider-btn"
              >
                {{ providerSubmitting ? '保存中...' : '确认创建' }}
              </UiButton>
            </div>
          </form>

          <!-- Google Antigravity OAuth 专属表单 -->
          <form v-else class="space-y-4" @submit.prevent="handleAuthorizeAntigravity">
            <!-- 极简本地代理状态检测：无背景无边框无注释，仅显示状态与重新探测 -->
            <div
              class="flex items-center justify-between text-[13px] py-1"
              data-testid="proxy-status-capsule"
            >
              <div class="flex items-center gap-2">
                <span class="relative flex h-2 w-2">
                  <span
                    class="animate-ping absolute inline-flex h-full w-full rounded-full opacity-75"
                    :class="proxyStatus?.available ? 'bg-emerald-400' : 'bg-amber-400'"
                  ></span>
                  <span
                    class="relative inline-flex rounded-full h-2 w-2"
                    :class="proxyStatus?.available ? 'bg-emerald-500' : 'bg-amber-500'"
                  ></span>
                </span>
                <span class="font-medium text-slate-700">
                  {{ proxyStatus?.description || '本地代理状态探测中...' }}
                </span>
                <span v-if="proxyStatus?.latency_ms" class="text-xs px-1.5 py-0.5 rounded bg-emerald-50 text-emerald-700 font-mono">
                  {{ proxyStatus.latency_ms }}ms
                </span>
              </div>
              <UiButton
                type="button"
                size="sm"
                variant="ghost"
                :disabled="probingProxy"
                class="text-[12px] h-7 px-2 text-slate-600 hover:text-slate-900"
                data-testid="probe-proxy-btn"
                @click="probeProxy"
              >
                <Icons name="refresh" size="12" :class="{ 'animate-spin': probingProxy }" />
                {{ probingProxy ? '探测中' : '重新探测' }}
              </UiButton>
            </div>

            <div class="grid grid-cols-1 sm:grid-cols-3 gap-3.5">
              <div>
                <label class="block text-[13px] font-medium text-slate-700 mb-1.5">服务商标识 *</label>
                <input
                  v-model="antigravityForm.provider"
                  type="text"
                  placeholder="antigravity"
                  required
                  class="w-full bg-slate-50 border border-slate-200 rounded-lg px-3.5 py-2 text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-400 focus:bg-white"
                  data-testid="ag-provider-input"
                />
              </div>

              <div class="sm:col-span-2">
                <label class="block text-[13px] font-medium text-slate-700 mb-1.5">Base URL (官方反代端点)</label>
                <input
                  value="https://daily-cloudcode-pa.googleapis.com"
                  type="text"
                  disabled
                  class="w-full bg-slate-100 border border-slate-200/80 rounded-lg px-3.5 py-2 text-sm text-slate-500 cursor-not-allowed"
                />
              </div>
            </div>

            <!-- OAuth 2.0 授权引导步骤 -->
            <div class="bg-slate-50 border border-slate-200/90 rounded-xl p-4.5 space-y-3.5">
              <div class="flex items-center justify-between">
                <span class="text-[14px] font-semibold text-slate-800 flex items-center gap-2">
                  <span class="flex items-center justify-center w-5 h-5 rounded-full bg-slate-900 text-white text-xs font-bold">1</span>
                  前往 Google 授权
                </span>
                <div class="flex items-center gap-2">
                  <UiButton
                    type="button"
                    size="sm"
                    :disabled="fetchingAuthUrl"
                    class="bg-indigo-600 hover:bg-indigo-700 text-white shadow-xs font-medium"
                    data-testid="ag-fetch-url-btn"
                    @click="fetchAndOpenAuthUrl"
                  >
                    <Icons name="external" size="14" />
                    {{ fetchingAuthUrl ? '生成中...' : '前往 Google 授权' }}
                  </UiButton>
                  <UiButton
                    v-if="antigravityAuthUrl"
                    type="button"
                    size="sm"
                    variant="ghost"
                    data-testid="ag-copy-url-btn"
                    @click="copyAuthUrl"
                  >
                    <Icons name="copy" size="14" />
                    {{ authUrlCopied ? '已复制！' : '复制授权链接' }}
                  </UiButton>
                </div>
              </div>

              <!-- 自动授权监听动态状态条 -->
              <div
                v-if="oauthWaiting"
                class="flex items-center justify-between p-3 bg-indigo-50/80 border border-indigo-200/70 rounded-lg text-[13px] text-indigo-900 animate-pulse"
                data-testid="ag-waiting-indicator"
              >
                <div class="flex items-center gap-2">
                  <Icons name="refresh" size="14" class="animate-spin text-indigo-600" />
                  <span>已打开授权弹窗，正在等待 Google 回调完成并自动换票...</span>
                </div>
                <button
                  type="button"
                  class="text-xs text-indigo-600 hover:text-indigo-800 underline cursor-pointer font-medium"
                  @click="cleanupOAuthSession"
                >
                  取消自动等待
                </button>
              </div>

              <!-- 自动生成的授权链接（多行展示，完全展现，只读禁止编辑） -->
              <div v-if="antigravityAuthUrl" class="space-y-1">
                <label class="block text-xs text-slate-500 font-medium">授权链接 (系统自动生成)</label>
                <textarea
                  :value="antigravityAuthUrl"
                  readonly
                  rows="3"
                  class="w-full text-xs text-slate-600 bg-white p-2.5 rounded-lg border border-slate-200 font-mono break-all select-all resize-none focus:outline-none cursor-default"
                ></textarea>
              </div>

              <div class="space-y-1.5 pt-1">
                <div class="text-[14px] font-semibold text-slate-800 flex items-center gap-2">
                  <span class="flex items-center justify-center w-5 h-5 rounded-full bg-slate-900 text-white text-xs font-bold">2</span>
                  重定向 URL 或 Code *
                </div>
                <p class="text-[13px] text-slate-500 leading-relaxed">
                  页面将在授权后自动完成授权并入池。如自动授权没有完成，请将浏览器地址栏中的完整重定向链接（以 <code>/oauth2callback</code> 结尾并带有参数）或授权码（<code>code=</code> 后的凭证）复制粘贴至下方输入框，点击“确认授权”。
                </p>
                <input
                  v-model="antigravityForm.code_or_url"
                  type="text"
                  placeholder="例如: http://localhost:8080/oauth2callback?code=4/0A... 或纯 Code"
                  required
                  class="w-full bg-white border border-slate-200 rounded-lg px-3.5 py-2 text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-400"
                  data-testid="ag-code-input"
                />
              </div>

              <div class="pt-1">
                <button
                  type="button"
                  class="text-[13px] font-semibold text-slate-700 hover:text-slate-900 inline-flex items-center gap-1.5 cursor-pointer py-0.5 select-none"
                  @click="showAgAdvanced = !showAgAdvanced"
                >
                  <Icons :name="showAgAdvanced ? 'chevron-down' : 'chevron-right'" size="13" />
                  高级选项 (自定义 Key ID 与调度权重)
                </button>

                <UiCollapsible :open="showAgAdvanced">
                  <div class="grid grid-cols-1 sm:grid-cols-3 gap-3 pt-2 p-3.5 bg-white rounded-lg border border-slate-200 mt-2">
                    <div>
                      <label class="block text-xs text-slate-600 mb-1 font-medium">Key 标识 (选填)</label>
                      <input
                        v-model="antigravityForm.id"
                        type="text"
                        placeholder="留空自动以 Google 邮箱命名"
                        class="w-full bg-slate-50 border border-slate-200 rounded-lg px-3 py-1.5 text-sm text-slate-900"
                        data-testid="ag-custom-id-input"
                      />
                    </div>
                    <div>
                      <label class="block text-xs text-slate-600 mb-1 font-medium">优先级</label>
                      <input
                        v-model.number="antigravityForm.priority"
                        type="number"
                        min="0"
                        class="w-full bg-slate-50 border border-slate-200 rounded-lg px-3 py-1.5 text-sm text-slate-900"
                        data-testid="ag-priority-input"
                      />
                    </div>
                    <div>
                      <label class="block text-xs text-slate-600 mb-1 font-medium">权重</label>
                      <input
                        v-model.number="antigravityForm.weight"
                        type="number"
                        min="1"
                        class="w-full bg-slate-50 border border-slate-200 rounded-lg px-3 py-1.5 text-sm text-slate-900"
                        data-testid="ag-weight-input"
                      />
                    </div>
                  </div>
                </UiCollapsible>
              </div>
            </div>

            <div class="flex items-center justify-end gap-2 pt-3 border-t border-slate-100">
              <UiButton variant="ghost" size="sm" @click="cancelAddProvider">
                取消
              </UiButton>
              <UiButton
                type="submit"
                size="sm"
                :disabled="providerSubmitting || !adminWriteEnabled"
                data-testid="submit-ag-provider-btn"
              >
                {{ providerSubmitting ? '正在授权...' : '确认授权' }}
              </UiButton>
            </div>
          </form>
        </div>
      </UiCollapsible>

      <!-- 视图与导航微标签 (兼具分类过滤与测试兼容) -->
      <div class="flex items-center gap-2 mb-6 border-b border-white/50 pb-2">
        <button
          type="button"
          class="px-4 py-2 text-[14px] font-medium rounded-lg transition-colors cursor-pointer"
          :class="currentTab === 'providers' ? 'bg-white/70 shadow-2xs text-slate-950 font-semibold border border-white/60 backdrop-blur-xs' : 'text-slate-600 hover:text-slate-900 hover:bg-white/30'"
          data-testid="tab-providers"
          @click="currentTab = 'providers'"
        >
          全部服务商 ({{ providers.length }})
        </button>

        <button
          type="button"
          class="px-4 py-2 text-[14px] font-medium rounded-lg transition-colors cursor-pointer"
          :class="currentTab === 'strategy' ? 'bg-white/70 shadow-2xs text-slate-950 font-semibold border border-white/60 backdrop-blur-xs' : 'text-slate-600 hover:text-slate-900 hover:bg-white/30'"
          data-testid="tab-strategy"
          @click="currentTab = 'strategy'"
        >
          全局策略
        </button>
      </div>

      <!-- 主视图：按服务商一级卡片排列 (三合一架构) -->
      <div v-if="currentTab === 'providers'" class="space-y-4">
        <div v-if="providers.length === 0" class="text-center py-16 swiss-card">
          <Icons name="server" size="36" class="text-slate-300 mx-auto mb-2.5" />
          <p class="text-base font-semibold text-slate-800">暂无模型服务商</p>
          <p class="text-sm text-slate-500 mt-1 mb-4">点击上方「+ 服务商」按钮即可接入上游 LLM</p>
        </div>

        <template v-else>
          <ProviderCard
            v-for="p in providers"
            :key="p.name"
            :provider="p"
            :models="models.filter((m) => m.provider ? m.provider === p.name : true)"
            :keys="keys.filter((k) => k.provider === p.name)"
            :on-delete-model="removeModel"
            :on-batch-create="batchCreateModels"
            @notice="showToast"
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
            @oauth-antigravity="handleOpenAntigravityForProvider"
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
