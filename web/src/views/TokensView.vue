<script setup lang="ts">
// B003: user self-service token panel (JWT plane, /api/user/tokens).
import { onMounted, ref } from 'vue';
import UiCard from '../components/ui/UiCard.vue';
import UiButton from '../components/ui/UiButton.vue';
import UiBadge from '../components/ui/UiBadge.vue';
import Icons from '../components/ui/Icons.vue';
import NavBar from '../components/NavBar.vue';
import {
  listMyTokens,
  createMyToken,
  updateMyToken,
  deleteMyToken,
  rotateMyToken,
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
const modelLimits = ref('');
const quota = ref<number | null>(null);
const creating = ref(false);
const modalError = ref('');

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

onMounted(() => {
  void refresh();
});

function openCreateModal(): void {
  name.value = '';
  modelLimits.value = '';
  quota.value = null;
  modalError.value = '';
  showCreateModal.value = true;
}

function closeCreateModal(): void {
  if (creating.value) return;
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
    const limits = modelLimits.value
      .split(',')
      .map((s) => s.trim())
      .filter((s) => s !== '');
    const created = await createMyToken({
      name: trimmed,
      model_limits: limits.length > 0 ? limits : undefined,
      quota: quota.value ?? undefined,
    });
    // 保存后关闭编辑区，显示生成的 token 弹窗
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
          <p class="mt-1 text-xs text-slate-500">自助创建与管理仅限 LLM 调用使用的访问凭证（明文仅显示一次）。</p>
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
                <UiBadge :variant="token.enabled ? 'success' : 'secondary'">
                  {{ token.enabled ? '启用' : '停用' }}
                </UiBadge>
              </div>
              <div class="text-[11px] text-slate-500 font-mono truncate">{{ token.key_id }}</div>
              <div class="text-[11px] text-slate-500">
                模型：{{ fmtModels(token.model_limits) }} · 用量 {{ token.used_tokens }} / {{ fmtTokens(token.quota) }}
              </div>
            </div>
            <div class="flex items-center gap-1.5 shrink-0">
              <UiButton variant="secondary" size="sm" @click="handleToggle(token)">
                {{ token.enabled ? '停用' : '启用' }}
              </UiButton>
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
                名称（1-64 字符）<span class="text-rose-500">*</span>
              </label>
              <input
                id="token-name"
                v-model="name"
                type="text"
                placeholder="例如 ci-swe-token"
                class="w-full h-10 px-3 py-2 text-sm bg-slate-50/50 border border-slate-200 rounded-lg focus:outline-none focus:bg-white focus:border-slate-400 text-slate-900 placeholder:text-slate-400 transition-all"
                data-testid="token-name-input"
              />
            </div>

            <div>
              <label for="token-quota" class="block text-xs font-medium text-slate-700 mb-1.5">
                用量上限（Token 数，留空=不限）
              </label>
              <input
                id="token-quota"
                v-model.number="quota"
                type="number"
                min="0"
                placeholder="1000000"
                class="w-full h-10 px-3 py-2 text-sm bg-slate-50/50 border border-slate-200 rounded-lg focus:outline-none focus:bg-white focus:border-slate-400 text-slate-900 placeholder:text-slate-400 transition-all"
                data-testid="token-quota-input"
              />
            </div>

            <div>
              <label for="token-models" class="block text-xs font-medium text-slate-700 mb-1.5">
                模型白名单（逗号分隔，支持 provider/model 与 deepseek/* 通配；留空=全部）
              </label>
              <input
                id="token-models"
                v-model="modelLimits"
                type="text"
                placeholder="gpt-4o-mini, deepseek/*"
                class="w-full h-10 px-3 py-2 text-sm bg-slate-50/50 border border-slate-200 rounded-lg focus:outline-none focus:bg-white focus:border-slate-400 text-slate-900 placeholder:text-slate-400 transition-all font-mono tracking-wide"
                data-testid="token-models-input"
              />
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
          <div class="p-3 bg-emerald-50/80 border border-emerald-200/80 rounded-xl text-emerald-800 leading-relaxed text-xs">
            <strong class="font-medium">安全提醒：</strong>
            {{ resultToken.isRotate ? 'Token 已生成新明文凭证，旧凭证已即刻作废失效。' : '新凭证已生成。' }}
            出于安全考量，明文密钥仅在此时展示一次，后续无法再次查看。请立即复制并妥善保管！
          </div>

          <div>
            <label class="block text-xs font-medium text-slate-600 mb-1.5">
              访问凭据 (ID: {{ resultToken.keyId }})
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
            我已安全复制，关闭
          </UiButton>
        </div>
      </div>
    </div>
  </div>
</template>
