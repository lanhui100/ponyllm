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
            <label for="username-input" class="block text-xs font-medium text-slate-700 mb-1.5">
              用户名
            </label>
            <div class="relative">
              <input
                id="username-input"
                v-model="usernameInput"
                type="text"
                autocomplete="username"
                autocapitalize="none"
                autocorrect="off"
                spellcheck="false"
                placeholder="请输入用户名"
                class="w-full h-10 px-3 py-2 text-sm bg-slate-50/50 border border-slate-200 rounded-lg focus:outline-none focus:bg-white focus:border-slate-400 focus:ring-2 focus:ring-slate-400/20 text-slate-900 placeholder:text-slate-400 transition-all tracking-wide"
                :disabled="loading"
              />
            </div>
          </div>

          <div>
            <label for="password-input" class="block text-xs font-medium text-slate-700 mb-1.5">
              密码
            </label>
            <div class="relative">
              <input
                id="password-input"
                v-model="passwordInput"
                type="password"
                autocomplete="current-password"
                autocapitalize="none"
                autocorrect="off"
                spellcheck="false"
                placeholder="请输入密码"
                class="w-full h-10 px-3 py-2 text-sm bg-slate-50/50 border border-slate-200 rounded-lg focus:outline-none focus:bg-white focus:border-slate-400 focus:ring-2 focus:ring-slate-400/20 text-slate-900 placeholder:text-slate-400 transition-all tracking-wide"
                :disabled="loading"
              />
            </div>
          </div>

          <div v-if="upgradedNotice" class="p-2.5 rounded-lg bg-amber-50 border border-amber-200/60 text-xs text-amber-700 flex items-start gap-2 animate-in fade-in duration-200">
            <Icons name="info" size="14" class="mt-0.5 shrink-0 text-amber-500" />
            <span class="leading-relaxed">网关已发版，旧会话已清除，请重新登录。</span>
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
            <span>{{ loading ? '登录中...' : '登录' }}</span>
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
import { probeOpenMode } from '../router';
import UiCard from '../components/ui/UiCard.vue';
import UiButton from '../components/ui/UiButton.vue';
import Icons from '../components/ui/Icons.vue';
import PonyLogo from '../components/ui/PonyLogo.vue';

const usernameInput = ref('');
const passwordInput = ref('');
const error = ref('');
const loading = ref(false);
const openMode = ref(false);
const upgradedNotice = ref(false);
const route = useRoute();
const router = useRouter();
const session = useSessionStore();

onMounted(async () => {
  // P2 发版强制 logout: /health 免鉴权，取服务端版本比对；版本变更则清会话。
  try {
    const health = await fetch('/health');
    const body = (await health.json().catch(() => null)) as { version?: unknown } | null;
    const ver = typeof body?.version === 'string' ? body.version : '';
    if (ver !== '' && session.logoutIfGatewayUpgraded(ver)) {
      upgradedNotice.value = true;
    }
  } catch {
    // 网关不可达时不强制 logout，探针/表单提交会给出 DOWN/不可达态。
  }
  // If navigating to /connect explicitly, clear any stale session
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

/// Web 登录成功后的落点：默认落 /dashboard。
/// 若 URL 携带了合法重定向目标，优先前往重定向页面（如 /dashboard）。
async function enterUserHome(): Promise<void> {
  const target = route.query.redirect ? sanitizeRedirect(route.query.redirect) : '/dashboard';
  await router.push(target);
}

async function submit(): Promise<void> {
  if (loading.value) return;
  error.value = '';
  const username = usernameInput.value.trim();
  const password = passwordInput.value;

  if (username === '' || password === '') {
    error.value = '请输入用户名与密码';
    return;
  }

  loading.value = true;
  try {
    const { loginWithPassword } = await import('../lib/userApi');
    const resp = await loginWithPassword(username, password);
    session.loginWithJwt(resp.access_token, resp.user.role);
    await enterUserHome();
  } catch (loginErr) {
    const code = (loginErr as Error & { code?: unknown }).code;
    if (code === 'invalid_credentials') {
      error.value = '用户名或密码错误（401）。';
      return;
    }
    if (code === 'rate_limit_exceeded') {
      error.value = '登录尝试过于频繁，请稍后重试。';
      return;
    }
    error.value = `登录失败：${(loginErr as Error).message ?? '未知错误'}`;
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
