<script setup lang="ts">
// B003: user self-service token panel (JWT plane, /api/user/tokens).
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import UiCard from '../components/ui/UiCard.vue';
import UiButton from '../components/ui/UiButton.vue';
import UiBadge from '../components/ui/UiBadge.vue';
import Icons from '../components/ui/Icons.vue';
import NavBar from '../components/NavBar.vue';
import {
  listMyTokens,
  listAvailableModels,
  createMyToken,
  updateMyToken,
  deleteMyToken,
  rotateMyToken,
  type CreateTokenRequest,
  type UserModelOption,
  type UserTokenMeta,
} from '../lib/userApi';
import { useToast } from '../composables/useToast';

const rows = ref<UserTokenMeta[]>([]);
const loading = ref(true);
const error = ref('');
const { toast } = useToast();

// Create Modal state & form
const showCreateModal = ref(false);
const name = ref('');
const modelLimits = ref<string[]>([]);
const quota = ref<number | null>(null);
const creating = ref(false);
const modalError = ref('');

// wave-6：「允许模型」多选下拉（数据源 GET /api/user/models，失败不阻塞创建）
const models = ref<UserModelOption[]>([]);
const modelsLoading = ref(false);
const modelsError = ref('');
const modelSearch = ref('');
const modelPickerOpen = ref(false);
const modelPickerRoot = ref<HTMLElement | null>(null);

const filteredModels = computed<UserModelOption[]>(() => {
  const query = modelSearch.value.trim().toLowerCase();
  if (query === '') return models.value;
  return models.value.filter(
    (m) =>
      m.id.toLowerCase().includes(query) ||
      (m.display_name ?? '').toLowerCase().includes(query),
  );
});

// Token Result Modal state (for create & rotate)
const showResultModal = ref(false);
const resultToken = ref<{ keyId: string; apiKey: string; isRotate: boolean } | null>(null);
const copied = ref(false);

async function refresh(): Promise<void> {
  loading.value = true;
  error.value = '';
  try {
    rows.value = await listMyTokens();
  } catch (e) {
    error.value = (e as Error).message ?? '加载 Token 列表失败';
  } finally {
    loading.value = false;
  }
}

// wave-6 缺陷修复（C1/C2/C3）：单飞 + 代次守卫；面板关闭不取消在途请求（页面级缓存）。
let modelsInFlight: Promise<void> | null = null;
let modelsReqSeq = 0;

function loadModels(): Promise<void> {
  // C1 单飞：在途请求存在时复用同一 Promise，不再发起第二次调用。
  if (modelsInFlight) {
    return modelsInFlight;
  }
  const seq = ++modelsReqSeq;
  modelsLoading.value = true;
  modelsError.value = '';
  const request = (async () => {
    try {
      const list = await listAvailableModels();
      // C2 代次守卫：仅最新一代可回写；陈旧成功响应静默丢弃。
      if (seq !== modelsReqSeq) return;
      models.value = list;
      modelsError.value = '';
    } catch (e) {
      // C4 失败降级：失败不阻塞弹窗与提交，仅提示可重试。
      // C2 代次守卫：陈旧失败同样静默丢弃。
      if (seq !== modelsReqSeq) return;
      models.value = [];
      modelsError.value = (e as Error).message || '模型列表加载失败';
    } finally {
      if (seq === modelsReqSeq) {
        modelsLoading.value = false;
      }
    }
  })();
  const settled = request.finally(() => {
    if (modelsInFlight === settled) {
      modelsInFlight = null;
    }
  });
  modelsInFlight = settled;
  return settled;
}

function isModelSelected(id: string): boolean {
  return modelLimits.value.includes(id);
}

function toggleModel(id: string): void {
  modelLimits.value = isModelSelected(id)
    ? modelLimits.value.filter((item) => item !== id)
    : [...modelLimits.value, id];
}

function onModelPickerPointerDown(event: Event): void {
  const root = modelPickerRoot.value;
  const target = event.target;
  if (root && target instanceof Node && root.contains(target)) return;
  closeModelPicker();
}

function onModelPickerKeydown(event: KeyboardEvent): void {
  if (event.key === 'Escape') closeModelPicker();
}

function openModelPicker(): void {
  if (modelPickerOpen.value) return;
  modelPickerOpen.value = true;
  document.addEventListener('mousedown', onModelPickerPointerDown, true);
  document.addEventListener('keydown', onModelPickerKeydown, true);
}

function closeModelPicker(): void {
  modelPickerOpen.value = false;
  document.removeEventListener('mousedown', onModelPickerPointerDown, true);
  document.removeEventListener('keydown', onModelPickerKeydown, true);
}

function toggleModelPicker(): void {
  if (modelPickerOpen.value) closeModelPicker();
  else openModelPicker();
}

onMounted(() => {
  void refresh();
  void loadModels();
});

onBeforeUnmount(() => {
  closeModelPicker();
});

function openCreateModal(): void {
  name.value = '';
  modelLimits.value = [];
  modelSearch.value = '';
  closeModelPicker();
  quota.value = null;
  modalError.value = '';
  showCreateModal.value = true;
  void loadModels();
}

function closeCreateModal(): void {
  if (creating.value) return;
  closeModelPicker();
  showCreateModal.value = false;
  modalError.value = '';
}

async function handleCreate(): Promise<void> {
  if (creating.value) return;
  const trimmed = name.value.trim();
  if (trimmed === '') {
    modalError.value = '请输入 Token 名称';
    return;
  }
  creating.value = true;
  modalError.value = '';
  try {
    // 契约 §2.4：选中 ≥1 → model_limits: string[]；选中 0 → 键不得出现（undefined）。
    const payload: CreateTokenRequest = {
      name: trimmed,
      quota: quota.value ?? undefined,
    };
    if (modelLimits.value.length > 0) {
      payload.model_limits = [...modelLimits.value];
    }
    const created = await createMyToken(payload);
    // 保存后关闭编辑区，显示生成的 token 弹窗
    closeModelPicker();
    showCreateModal.value = false;
    resultToken.value = { keyId: created.key_id, apiKey: created.api_key, isRotate: false };
    copied.value = false;
    showResultModal.value = true;
    toast.success('Token 创建成功（明文仅显示一次）');
    await refresh();
  } catch (e) {
    modalError.value = (e as Error).message ?? '创建失败';
  } finally {
    creating.value = false;
  }
}

async function handleToggle(token: UserTokenMeta): Promise<void> {
  try {
    await updateMyToken(token.key_id, { enabled: !token.enabled });
    toast.success(token.enabled ? 'Token 已停用' : 'Token 已启用');
    await refresh();
  } catch (e) {
    error.value = (e as Error).message ?? '操作失败';
  }
}

async function handleRotate(token: UserTokenMeta): Promise<void> {
  try {
    const rotated = await rotateMyToken(token.key_id);
    resultToken.value = { keyId: token.key_id, apiKey: rotated.api_key, isRotate: true };
    copied.value = false;
    showResultModal.value = true;
    toast.success('Token 已轮换（新明文仅显示一次）');
    await refresh();
  } catch (e) {
    error.value = (e as Error).message ?? '轮换失败';
  }
}

async function handleDelete(token: UserTokenMeta): Promise<void> {
  try {
    await deleteMyToken(token.key_id);
    toast.success('Token 已删除');
    await refresh();
  } catch (e) {
    error.value = (e as Error).message ?? '删除失败';
  }
}

async function handleCopy(): Promise<void> {
  if (!resultToken.value?.apiKey) return;
  try {
    await navigator.clipboard.writeText(resultToken.value.apiKey);
    copied.value = true;
    toast.success('已复制到剪贴板');
    setTimeout(() => {
      copied.value = false;
    }, 2000);
  } catch {
    // 降级兜底方案
    try {
      const textarea = document.createElement('textarea');
      textarea.value = resultToken.value.apiKey;
      textarea.style.position = 'fixed';
      textarea.style.opacity = '0';
      document.body.appendChild(textarea);
      textarea.select();
      document.execCommand('copy');
      document.body.removeChild(textarea);
      copied.value = true;
      toast.success('已复制到剪贴板');
      setTimeout(() => {
        copied.value = false;
      }, 2000);
    } catch {
      toast.error('复制失败，请手动选取复制');
    }
  }
}

function closeResultModal(): void {
  showResultModal.value = false;
  resultToken.value = null;
  copied.value = false;
}

function fmtTokens(n: number | null): string {
  return n === null ? '∞' : String(n);
}

function fmtModels(list: string[] | null): string {
  return list && list.length > 0 ? list.join(', ') : '全部模型';
}
</script>

<template>
  <div class="min-h-screen">
    <NavBar />
    <main class="max-w-5xl mx-auto px-4 sm:px-8 py-8 space-y-6">
      <div class="flex items-center justify-between">
        <div>
          <h1 class="text-xl font-bold tracking-tight text-slate-900">我的 Token</h1>
          <p class="mt-1 text-xs text-slate-500">创建与管理 LLM 访问凭证。</p>
        </div>
      </div>

      <div v-if="error" class="p-2.5 rounded-lg bg-rose-50 border border-rose-200/60 text-xs text-rose-600">
        <span class="error">{{ error }}</span>
      </div>

      <!-- Token list -->
      <UiCard class="p-5">
        <div class="flex items-center justify-between mb-3">
          <h2 class="text-sm font-semibold text-slate-800">Token 列表</h2>
        </div>
        <div v-if="loading" class="text-xs text-slate-400 py-4">加载中...</div>
        <div v-else-if="rows.length === 0" class="text-xs text-slate-400 py-4">还没有 Token，请点击下方新增按钮创建。</div>
        <div v-else class="space-y-2.5">
          <div
            v-for="token in rows"
            :key="token.key_id"
            class="flex items-center justify-between gap-3 p-3 rounded-lg border border-slate-200/70 bg-slate-50/40"
          >
            <div class="min-w-0 space-y-1">
              <div class="flex items-center gap-2">
                <span class="text-sm font-medium text-slate-800 truncate">{{ token.name ?? '未命名' }}</span>
              </div>
              <div class="text-[11px] text-slate-500 font-mono truncate">{{ token.key_id }}</div>
              <div class="text-[11px] text-slate-500">
                模型：{{ fmtModels(token.model_limits) }} · 用量 {{ token.used_tokens }} / {{ fmtTokens(token.quota) }}
              </div>
            </div>
            <div class="flex items-center gap-1.5 shrink-0">
              <UiButton variant="outline" size="sm" data-testid="rotate-token-btn" @click="handleRotate(token)">
                <Icons name="refresh" size="13" />
                <span>轮换</span>
              </UiButton>
              <UiButton variant="destructive" size="sm" @click="handleDelete(token)">
                <span>删除</span>
              </UiButton>
            </div>
          </div>
        </div>

        <!-- 列表下方新增按钮 -->
        <div class="mt-4 pt-3 border-t border-slate-100 flex justify-start">
          <UiButton data-testid="open-create-token-btn" @click="openCreateModal">
            <Icons name="plus" size="14" />
            <span>新增 Token</span>
          </UiButton>
        </div>
      </UiCard>
    </main>

    <!-- 新增编辑弹窗 -->
    <div
      v-if="showCreateModal"
      class="fixed inset-0 z-50 flex items-center justify-center p-4 bg-slate-900/40 backdrop-blur-md"
      data-testid="create-token-modal"
    >
      <div class="bg-white/95 backdrop-blur-xl rounded-2xl shadow-2xl w-full max-w-lg overflow-hidden border border-slate-200/80">
        <div class="p-5 border-b border-slate-100 flex items-center justify-between">
          <h3 class="text-sm font-bold text-slate-900 flex items-center gap-2">
            <Icons name="key" size="16" class="text-indigo-600" />
            新增 Token
          </h3>
          <button
            type="button"
            class="text-slate-400 hover:text-slate-600 transition-colors p-1 rounded-lg"
            @click="closeCreateModal"
          >
            <Icons name="cross" size="16" />
          </button>
        </div>

        <div class="p-5 space-y-4">
          <div v-if="modalError" class="p-2.5 rounded-lg bg-rose-50 border border-rose-200/60 text-xs text-rose-600">
            {{ modalError }}
          </div>

          <div class="space-y-3">
            <div>
              <label for="token-name" class="block text-xs font-medium text-slate-700 mb-1.5">
                名称 <span class="text-rose-500">*</span>
              </label>
              <input
                id="token-name"
                v-model="name"
                type="text"
                placeholder="Token 名称"
                class="w-full h-10 px-3 py-2 text-sm bg-slate-50/50 border border-slate-200 rounded-lg focus:outline-none focus:bg-white focus:border-slate-400 text-slate-900 placeholder:text-slate-400 transition-all"
                data-testid="token-name-input"
              />
            </div>

            <div>
              <label for="token-quota" class="block text-xs font-medium text-slate-700 mb-1.5">
                用量上限
              </label>
              <input
                id="token-quota"
                v-model.number="quota"
                type="number"
                min="0"
                placeholder="留空为不限"
                class="w-full h-10 px-3 py-2 text-sm bg-slate-50/50 border border-slate-200 rounded-lg focus:outline-none focus:bg-white focus:border-slate-400 text-slate-900 placeholder:text-slate-400 transition-all"
                data-testid="token-quota-input"
              />
            </div>

            <div>
              <label id="token-models-label" class="block text-xs font-medium text-slate-700 mb-1.5">
                允许模型
              </label>
              <div ref="modelPickerRoot" class="relative">
                <button
                  type="button"
                  class="w-full min-h-10 px-3 py-2 text-sm bg-slate-50/50 border rounded-lg text-left flex items-center justify-between gap-2 focus:outline-none focus:bg-white transition-all"
                  :class="modelPickerOpen ? 'border-slate-400 bg-white' : 'border-slate-200'"
                  data-testid="token-models-select"
                  aria-haspopup="listbox"
                  aria-labelledby="token-models-label"
                  :aria-expanded="modelPickerOpen ? 'true' : 'false'"
                  @click.stop="toggleModelPicker"
                >
                  <span :class="modelLimits.length === 0 ? 'text-slate-400' : 'text-slate-900'">
                    {{ modelLimits.length === 0 ? '留空为全部模型' : `已选 ${modelLimits.length} 个模型` }}
                  </span>
                  <Icons name="chevron-down" size="14" class="text-slate-400" />
                </button>

                <!-- 已选模型 chips（每个 chip 带移除按钮） -->
                <div v-if="modelLimits.length > 0" class="mt-2 flex flex-wrap gap-1.5">
                  <span
                    v-for="id in modelLimits"
                    :key="id"
                    class="inline-flex items-center gap-1 pl-2 pr-1 py-0.5 rounded-lg bg-slate-100 border border-slate-200 text-[11px] text-slate-700"
                    data-testid="token-models-chip"
                  >
                    <span class="font-mono truncate max-w-[12rem]">{{ id }}</span>
                    <button
                      type="button"
                      class="text-slate-400 hover:text-slate-600 transition-colors rounded p-0.5"
                      :aria-label="`移除模型 ${id}`"
                      @click.stop="toggleModel(id)"
                    >
                      <Icons name="cross" size="10" />
                    </button>
                  </span>
                </div>

                <!-- 下拉浮层：搜索 + 选项列表（关闭态仅隐藏，保证选项与选中态同步）
                     缺陷修复 B：向上展开（bottom-full mb-1），避免被 modal card 的
                     overflow-hidden 裁掉；列表加保守高度上限防矮视口溢出卡片顶部。 -->
                <div
                  v-show="modelPickerOpen"
                  class="absolute z-20 left-0 right-0 bottom-full mb-1 rounded-lg border border-slate-200 bg-white shadow-xl overflow-hidden"
                >
                  <div class="p-2 border-b border-slate-100">
                    <input
                      v-model="modelSearch"
                      type="text"
                      placeholder="搜索模型 id 或名称"
                      class="w-full h-8 px-2 text-xs bg-slate-50/50 border border-slate-200 rounded-lg focus:outline-none focus:bg-white focus:border-slate-400 text-slate-900 placeholder:text-slate-400"
                      data-testid="token-models-search"
                    />
                  </div>
                  <p v-if="modelsLoading" class="px-3 py-2 text-[11px] text-slate-400">加载中...</p>
                  <p
                    v-else-if="filteredModels.length === 0"
                    class="px-3 py-2 text-[11px] text-slate-400"
                  >
                    没有匹配的模型
                  </p>
                  <div class="max-h-48 overflow-y-auto p-1" role="listbox" aria-multiselectable="true">
                    <button
                      v-for="m in filteredModels"
                      :key="m.id"
                      type="button"
                      role="option"
                      :aria-selected="isModelSelected(m.id) ? 'true' : 'false'"
                      class="w-full text-left px-2 py-1.5 rounded-lg hover:bg-slate-50 flex items-center gap-2 transition-colors"
                      data-testid="token-models-option"
                      @click.stop="toggleModel(m.id)"
                    >
                      <span
                        class="w-3.5 h-3.5 shrink-0 rounded border flex items-center justify-center"
                        :class="isModelSelected(m.id) ? 'bg-indigo-600 border-indigo-600 text-white' : 'border-slate-300'"
                      >
                        <Icons v-if="isModelSelected(m.id)" name="check" size="9" />
                      </span>
                      <span class="min-w-0 flex-1">
                        <span class="block text-xs text-slate-800 font-mono truncate">{{ m.id }}</span>
                        <span v-if="m.display_name" class="block text-[11px] text-slate-400 truncate">
                          {{ m.display_name }}
                        </span>
                      </span>
                      <span v-if="m.provider" class="text-[11px] text-slate-400 shrink-0">{{ m.provider }}</span>
                    </button>
                  </div>
                </div>
              </div>

              <!-- 契约 §2.5：列表加载失败的可重试提示（不阻塞弹窗与提交） -->
              <div v-if="modelsError" class="mt-2 flex items-center gap-2 text-[11px] text-amber-600">
                <span>模型列表加载失败，可留空提交或重试。</span>
                <button
                  type="button"
                  class="text-indigo-600 hover:text-indigo-700 transition-colors"
                  data-testid="token-models-retry"
                  @click="loadModels"
                >
                  重试
                </button>
              </div>
            </div>
          </div>
        </div>

        <div class="p-4 bg-slate-50 border-t border-slate-100 flex items-center justify-end gap-2">
          <UiButton variant="ghost" size="sm" :disabled="creating" @click="closeCreateModal">
            取消
          </UiButton>
          <UiButton :disabled="creating" size="sm" data-testid="submit-create-token-btn" @click="handleCreate">
            <Icons v-if="creating" name="refresh" size="14" class="animate-spin" />
            <Icons v-else name="plus" size="14" />
            <span>{{ creating ? '保存中...' : '保存' }}</span>
          </UiButton>
        </div>
      </div>
    </div>

    <!-- 生成/轮换后 Token 展示与复制弹窗 -->
    <div
      v-if="showResultModal && resultToken"
      class="fixed inset-0 z-50 flex items-center justify-center p-4 bg-slate-900/40 backdrop-blur-md"
      data-testid="token-result-modal"
    >
      <div class="bg-white/95 backdrop-blur-xl rounded-2xl shadow-2xl w-full max-w-lg overflow-hidden border border-slate-200/80">
        <div class="p-5 border-b border-slate-100 flex items-center justify-between">
          <h3 class="text-sm font-bold text-slate-900 flex items-center gap-2">
            <Icons name="key" size="16" class="text-emerald-600" />
            {{ resultToken.isRotate ? 'Token 轮换成功' : 'Token 创建成功' }}
          </h3>
          <UiBadge variant="success">明文仅展示一次</UiBadge>
        </div>

        <div class="p-5 space-y-4 text-sm">
          <div>
            <label class="block text-xs font-medium text-slate-600 mb-1.5">
              访问凭据
            </label>
            <div class="flex items-center gap-2">
              <input
                type="text"
                readonly
                :value="resultToken.apiKey"
                class="flex-1 font-mono text-xs sm:text-sm bg-slate-50 border border-slate-200 rounded-lg px-3 py-2 text-slate-900 select-all"
                data-testid="plaintext-token-input"
              />
              <UiButton
                variant="outline"
                size="sm"
                data-testid="copy-token-btn"
                @click="handleCopy"
              >
                <Icons :name="copied ? 'check' : 'copy'" size="14" />
                <span>{{ copied ? '已复制' : '复制' }}</span>
              </UiButton>
            </div>
          </div>
        </div>

        <div class="p-4 bg-slate-50 border-t border-slate-100 flex justify-end">
          <UiButton
            size="sm"
            data-testid="close-result-modal-btn"
            @click="closeResultModal"
          >
            关闭
          </UiButton>
        </div>
      </div>
    </div>
  </div>
</template>
