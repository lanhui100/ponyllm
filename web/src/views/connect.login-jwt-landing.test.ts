// @vitest-environment happy-dom
// Connect.vue JWT 登录落点契约测试（登录成功默认落 /dashboard）。
// 契约：
// - W1: loginWithPassword 返回 role=admin 的 user → router 默认落 /dashboard；
// - W2: role=user → 默认同样落 /dashboard（登录成功统一默认页）。
// 防 Flaky 律（test-expert）：挂载/提交/导航结算一律用"有界条件轮询（waitFor）"，
// 禁止硬编码 sleep —— 全量并行下 setTimeout(N) 不可靠（曾偶发时序竞态）。

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { createApp, nextTick } from 'vue';
import { createPinia, setActivePinia } from 'pinia';
import { createRouter, createMemoryHistory } from 'vue-router';
import ConnectView from './Connect.vue';

const DASHBOARD = { template: '<div>dashboard</div>' };
const TOKENS = { template: '<div>tokens</div>' };
let mountedApp: ReturnType<typeof createApp> | null = null;

/// 有界条件轮询（防 Flaky）：每 20ms 复查，条件满足即返回；超时抛错（非盲目 sleep）。
async function waitFor(cond: () => boolean, what: string, timeoutMs = 2000): Promise<void> {
  const start = Date.now();
  while (!cond()) {
    if (Date.now() - start > timeoutMs) {
      throw new Error(`waitFor 超时（${timeoutMs}ms）：${what}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
}

/// /health → 200（发版比对）；`__pony-probe__` → 401（open-mode 探针判定为关闭）；
/// /api/user/login → 按 loginRole 返回 JWT 登录响应；其余 → 200（页面渲染兜底）。
function stubFetch(calls: { url: string; init?: RequestInit }[], loginRole: 'admin' | 'user') {
  vi.stubGlobal('fetch', async (input: RequestInfo | URL, init?: RequestInit) => {
    const url =
      typeof input === 'string'
        ? input
        : input instanceof URL
          ? input.toString()
          : input.url;
    calls.push({ url, init });
    if (url === '/health') {
      return new Response(JSON.stringify({ version: '0.0.0' }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      });
    }
    const auth = (init?.headers as Record<string, string> | undefined)?.['Authorization'] ?? '';
    if (auth.includes('__pony-probe__')) {
      return new Response(JSON.stringify({ error: 'unauthorized' }), {
        status: 401,
        headers: { 'Content-Type': 'application/json' },
      });
    }
    if (url === '/api/user/login') {
      const isAdmin = loginRole === 'admin';
      return new Response(
        JSON.stringify({
          access_token: isAdmin ? 'jwt-admin-token' : 'jwt-user-token',
          user: {
            id: isAdmin ? 'usr-admin' : 'usr-alice',
            username: isAdmin ? 'admin' : 'alice',
            role: loginRole,
            name: isAdmin ? 'admin' : 'alice',
            enabled: true,
          },
        }),
        { status: 200, headers: { 'Content-Type': 'application/json' } },
      );
    }
    return new Response(JSON.stringify({}), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    });
  });
}

function mountConnect() {
  const pinia = createPinia();
  setActivePinia(pinia);
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/connect', component: ConnectView },
      { path: '/dashboard', component: DASHBOARD },
      { path: '/tokens', component: TOKENS },
    ],
  });
  const app = createApp(ConnectView);
  mountedApp = app;
  app.use(router);
  app.use(pinia);
  const container = document.createElement('div');
  document.body.appendChild(container);
  app.mount(container);
  return { app, container, router };
}

async function submitUsernamePassword(
  container: HTMLElement,
  username: string,
  password: string,
) {
  const userInput = container.querySelector('#username-input') as HTMLInputElement | null;
  if (userInput) {
    userInput.value = username;
    userInput.dispatchEvent(new Event('input'));
  }
  const passInput = container.querySelector('#password-input') as HTMLInputElement | null;
  if (passInput) {
    passInput.value = password;
    passInput.dispatchEvent(new Event('input'));
  }
  await nextTick();
  const form = container.querySelector('form') as HTMLFormElement;
  form.dispatchEvent(new Event('submit'));
  // 结算等待由调用方用有界条件轮询完成（本函数不做盲目 sleep）
}

describe('Connect.vue: JWT 登录落点角色感知（B005 W1/W2）', () => {
  beforeEach(() => {
    window.sessionStorage?.clear();
  });
  afterEach(() => {
    try {
      mountedApp?.unmount();
    } catch {
      /* noop */
    }
    mountedApp = null;
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    document.body.innerHTML = '';
  });

  it('W1: role=admin 登录成功 → 落 /dashboard（红相：当前恒落 /tokens → FAIL）', async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    stubFetch(calls, 'admin');
    const { container, router } = mountConnect();
    await waitFor(() => container.querySelector('form') !== null, '登录表单渲染');

    await submitUsernamePassword(container, 'admin', 'admin-pass-1234');

    await waitFor(
      () =>
        calls.some((c) => c.url === '/api/user/login') ||
        container.querySelector('.error') !== null,
      '提交结算（login 调用或错误渲染）',
    );
    // 导航结算：登录成功后 enterUserHome() 必须离开 /connect（初始 currentRoute 为 /）
    await waitFor(() => router.currentRoute.value.path !== '/', '登录后导航结算');

    expect(
      calls.some((c) => c.url === '/api/user/login'),
      'admin 登录必须 POST /api/user/login',
    ).toBe(true);
    expect(router.currentRoute.value.path).toBe('/dashboard');
  });

  it('W2: role=user 登录成功 → 默认落 /dashboard', async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    stubFetch(calls, 'user');
    const { container, router } = mountConnect();
    await waitFor(() => container.querySelector('form') !== null, '登录表单渲染');

    await submitUsernamePassword(container, 'alice', 'alice-pass-1234');

    await waitFor(
      () =>
        calls.some((c) => c.url === '/api/user/login') ||
        container.querySelector('.error') !== null,
      '提交结算（login 调用或错误渲染）',
    );
    await waitFor(() => router.currentRoute.value.path !== '/', '登录后导航结算');

    expect(
      calls.some((c) => c.url === '/api/user/login'),
      'user 登录必须 POST /api/user/login',
    ).toBe(true);
    expect(router.currentRoute.value.path).toBe('/dashboard');
  });
});
