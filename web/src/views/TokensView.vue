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

// Create form
const name = ref('');
const modelLimits = ref('');
const quota = ref<number | null>(null);
const creating = ref(false);

// One-time plaintext surface (cleared after display)
const lastPlaintext = ref<{ keyId: string; apiKey: string } | null>(null);
const lastRotated = ref<{ keyId: string; apiKey: string } | null>(null);

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

async function handleCreate(): Promise<void> {
  if (creating.value) return;
  const trimmed = name.value.trim();
  if (trimmed === '') {
    error.value = '请输入 Token 名称';
    return;
  }
  creating.value = true;
  error.value = '';
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
    // 明文仅一次：立即展示，复制后由用户关闭/下次操作清空。
    lastPlaintext.value = { keyId: created.key_id, apiKey: created.api_key };
    name.value = '';
    modelLimits.value = '';
    quota.value = null;
    toast.success('Token 创建成功（明文仅显示一次）');
    await refresh();
  } catch (e) {
    error.value = (e as Error).message ?? '创建失败';
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
    lastRotated.value = { keyId: token.key_id, apiKey: rotated.api_key };
    toast.success('Token 已旋转（新明文仅显示一次）');
    await refresh();
  } catch (e) {
    error.value = (e as Error).message ?? '旋转失败';
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

function clearPlaintext(): void {
  lastPlaintext.value = null;
  lastRotated.value = null;
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

      <!-- One-time plaintext surfaces -->
      <UiCard v-if="lastPlaintext || lastRotated" class="p-4 border-emerald-200/70 bg-emerald-50/60">
        <div class="flex items-start gap-3">
          <Icons name="info" size="16" class="mt-0.5 shrink-0 text-emerald-600" />
          <div class="flex-1 space-y-1.5 min-w-0">
            <p class="text-xs font-semibold text-emerald-800">
              {{ lastPlaintext ? '新 Token 明文（仅此一次，请立即保存）' : '旋转后的新明文（仅此一次）' }}
            </p>
            <p class="text-[11px] text-emerald-700 break-all font-mono leading-relaxed select-all">
              {{ lastPlaintext?.apiKey ?? lastRotated?.apiKey }}
            </p>
          </div>
          <UiButton variant="ghost" size="sm" @click="clearPlaintext">
            <span>关闭</span>
          </UiButton>
        </div>
      </UiCard>

      <!-- Create form -->
      <UiCard class="p-5">
        <h2 class="text-sm font-semibold text-slate-800 mb-3">创建 Token</h2>
        <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <div>
            <label for="token-name" class="block text-xs font-medium text-slate-700 mb-1.5">名称（1-64 字符）</label>
            <input
              id="token-name"
              v-model="name"
              type="text"
              placeholder="例如 ci-swe-token"
              class="w-full h-10 px-3 py-2 text-sm bg-slate-50/50 border border-slate-200 rounded-lg focus:outline-none focus:bg-white focus:border-slate-400 text-slate-900 placeholder:text-slate-400 transition-all"
            />
          </div>
          <div>
            <label for="token-quota" class="block text-xs font-medium text-slate-700 mb-1.5">用量上限（Token 数，留空=不限）</label>
            <input
              id="token-quota"
              v-model.number="quota"
              type="number"
              min="0"
              placeholder="1000000"
              class="w-full h-10 px-3 py-2 text-sm bg-slate-50/50 border border-slate-200 rounded-lg focus:outline-none focus:bg-white focus:border-slate-400 text-slate-900 placeholder:text-slate-400 transition-all"
            />
          </div>
          <div class="sm:col-span-2">
            <label for="token-models" class="block text-xs font-medium text-slate-700 mb-1.5">
              模型白名单（逗号分隔，支持 provider/model 与 deepseek/* 通配；留空=全部）
            </label>
            <input
              id="token-models"
              v-model="modelLimits"
              type="text"
              placeholder="gpt-4o-mini, deepseek/*"
              class="w-full h-10 px-3 py-2 text-sm bg-slate-50/50 border border-slate-200 rounded-lg focus:outline-none focus:bg-white focus:border-slate-400 text-slate-900 placeholder:text-slate-400 transition-all font-mono tracking-wide"
            />
          </div>
        </div>
        <div class="mt-4 flex justify-end">
          <UiButton :disabled="creating" @click="handleCreate">
            <Icons v-if="creating" name="refresh" size="14" class="animate-spin" />
            <Icons v-else name="plus" size="14" />
            <span>{{ creating ? '创建中...' : '创建 Token' }}</span>
          </UiButton>
        </div>
      </UiCard>

      <!-- Token list -->
      <UiCard class="p-5">
        <h2 class="text-sm font-semibold text-slate-800 mb-3">Token 列表</h2>
        <div v-if="loading" class="text-xs text-slate-400 py-4">加载中...</div>
        <div v-else-if="rows.length === 0" class="text-xs text-slate-400 py-4">还没有 Token，请在上方创建。</div>
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
              <UiButton variant="outline" size="sm" @click="handleRotate(token)">
                <span>旋转</span>
              </UiButton>
              <UiButton variant="destructive" size="sm" @click="handleDelete(token)">
                <span>删除</span>
              </UiButton>
            </div>
          </div>
        </div>
      </UiCard>
    </main>
  </div>
</template>
