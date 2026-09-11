<template>
  <div class="connect-page flex min-h-screen flex-col justify-center py-12 sm:px-6 lg:px-8 bg-transparent relative overflow-hidden select-none">
    <!-- Ambient background gradient overlay -->
    <div class="pointer-events-none absolute -top-40 left-1/2 -translate-x-1/2 w-[700px] h-[350px] bg-gradient-to-b from-orange-200/40 via-indigo-100/20 to-transparent blur-3xl" />

    <div class="sm:mx-auto sm:w-full sm:max-w-md relative z-10">
      <!-- Brand icon & title -->
      <div class="flex items-center justify-center gap-2.5 mb-2">
        <PonyLogo :size="40" />
        <span class="font-bold text-xl tracking-tight text-slate-900">PonyLLM</span>
      </div>
      <h2 class="text-center text-xl font-bold tracking-tight text-slate-800">
        连接网关
      </h2>
      <p class="mt-1.5 text-center text-xs text-slate-500">
        轻量高效的统一大模型代理与遥测控制台
      </p>
    </div>

    <div class="mt-8 sm:mx-auto sm:w-full sm:max-w-[420px] px-4 sm:px-0 relative z-10">
      <UiCard class="p-6 sm:p-8 shadow-sm border border-white/50 bg-white/55 backdrop-blur-xs">
        <div v-if="openMode" class="text-center py-4 space-y-4">
          <div class="w-12 h-12 rounded-full bg-emerald-50 text-emerald-600 flex items-center justify-center mx-auto border border-emerald-100">
            <Icons name="check" size="22" />
          </div>
          <div>
            <h3 class="text-sm font-semibold text-slate-800">网关处于免鉴权模式</h3>
            <p class="mt-1 text-xs text-slate-500">
              当前网关未配置 API Key，无需验证即可直接管理与查看。
            </p>
          </div>
          <UiButton
            variant="default"
            class="w-full mt-2"
            @click="enterDashboard"
          >
            进入控制台
          </UiButton>
        </div>

        <form v-else class="space-y-4" @submit.prevent="submit">
          <div>
            <div class="flex items-center justify-between mb-1.5">
              <label for="token-input" class="block text-xs font-medium text-slate-700">
                访问凭证 (Token)
              </label>
              <span class="text-[11px] text-slate-400">sk-pony-...</span>
            </div>
            <div class="relative">
              <input
                id="token-input"
                v-model="input"
                type="password"
                autocomplete="new-password"
                autocapitalize="none"
                autocorrect="off"
                spellcheck="false"
                placeholder="请输入网关访问 API Key"
                class="w-full h-10 px-3 py-2 text-sm bg-slate-50/50 border border-slate-200 rounded-lg focus:outline-none focus:bg-white focus:border-slate-400 focus:ring-2 focus:ring-slate-400/20 text-slate-900 placeholder:text-slate-400 transition-all font-mono tracking-wide"
                :disabled="loading"
              />
            </div>
          </div>

          <div v-if="error" class="p-2.5 rounded-lg bg-rose-50 border border-rose-200/60 text-xs text-rose-600 flex items-start gap-2 animate-in fade-in duration-200">
            <Icons name="info" size="14" class="mt-0.5 shrink-0 text-rose-500" />
            <span class="error leading-relaxed">{{ error }}</span>
          </div>

          <UiButton
            type="submit"
            variant="default"
            size="default"
            class="w-full h-10 text-sm font-medium gap-2 shadow-sm"
            :disabled="loading"
          >
            <Icons v-if="loading" name="refresh" size="14" class="animate-spin" />
            <Icons v-else name="lock" size="14" />
            <span>{{ loading ? '连接中...' : '连接' }}</span>
          </UiButton>
        </form>

        <div class="mt-6 pt-5 border-t border-slate-100 flex items-center justify-between text-[11px] text-slate-400">
          <span>PonyLLM Console</span>
          <span class="font-mono">Session Auth</span>
        </div>
      </UiCard>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import { useSessionStore } from '../stores/session';
import { bearerValue } from '../lib/alova';
import { PROBE_PATH, probeOpenMode } from '../router';
import UiCard from '../components/ui/UiCard.vue';
import UiButton from '../components/ui/UiButton.vue';
import Icons from '../components/ui/Icons.vue';
import PonyLogo from '../components/ui/PonyLogo.vue';

const input = ref('');
const error = ref('');
const loading = ref(false);
const openMode = ref(false);
const route = useRoute();
const router = useRouter();
const session = useSessionStore();

onMounted(async () => {
  const qToken = (route.query.token || route.query.key) as string | undefined;
  if (qToken && typeof qToken === 'string' && qToken.trim() !== '') {
    input.value = qToken.trim();
    await submit();
    return;
  }
  // If navigating to /connect explicitly without token in URL, clear any stale session
  session.logout();
  openMode.value = await probeOpenMode().catch(() => false);
});

function sanitizeRedirect(target: unknown): string {
  if (typeof target !== 'string') {
    return '/dashboard';
  }
  const trimmed = target.trim();
  // Must be an internal path starting with a single slash; strictly disallow protocol-relative,
  // backslash or URI scheme traversal (e.g. "//evil.com", "/\\evil.com", "javascript:", "https:")
  if (!trimmed.startsWith('/') || trimmed.startsWith('//') || trimmed.startsWith('/\\') || trimmed.includes('://')) {
    return '/dashboard';
  }
  return trimmed;
}

async function enterDashboard(): Promise<void> {
  const safeTarget = sanitizeRedirect(route.query.redirect);
  await router.push(safeTarget);
}

async function submit(): Promise<void> {
  if (loading.value) return;
  error.value = '';
  const candidate = input.value.trim();
  if (candidate === '') {
    error.value = '请输入 Token';
    return;
  }
  loading.value = true;
  try {
    const resp = await fetch(PROBE_PATH, {
      headers: { Authorization: bearerValue(candidate) },
    });
    // Tight: only 2xx logs in (P1-4). 401 => bad token; anything else (403/404/
    // 500) surfaces its status instead of silently "succeeding".
    if (resp.status === 401) {
      error.value = 'Token 无效（401）';
      return;
    }
    if (!resp.ok) {
      error.value = `网关异常（${resp.status}）`;
      return;
    }
    session.login(candidate);
    await enterDashboard();
  } catch {
    error.value = '网关不可达';
  } finally {
    loading.value = false;
  }
}
</script>

<style scoped>
.connect-page {
  /* Inherit global modern typography */
}
</style>
