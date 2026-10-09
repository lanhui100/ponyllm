// @vitest-environment happy-dom
// B003 契约测试：Connect.vue 登录表单改 username/password 双字段 + 提交走
// /api/user/login（JWT 会话）。
// 防 Flaky 律（test-expert）：挂载/提交结算一律用"显式条件轮询（有界超时）"等待，
// 禁止硬编码 sleep —— 全量并行下 setTimeout(N) 不可靠（曾偶发时序竞态）。

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { createApp, nextTick } from 'vue';
import { createPinia, setActivePinia } from 'pinia';
import { createRouter, createMemoryHistory } from 'vue-router';
import ConnectView from './Connect.vue';

const DASHBOARD = { template: '<div>dashboard</div>' };
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

function stubFetch(calls: { url: string; init?: RequestInit }[]) {
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
    return new Response(JSON.stringify({}), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    });
  });
}

function mountConnect(calls: { url: string; init?: RequestInit }[]) {
  const pinia = createPinia();
  setActivePinia(pinia);
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/connect', component: ConnectView },
      { path: '/dashboard', component: DASHBOARD },
      { path: '/', redirect: '/dashboard' },
    ],
  });
  const app = createApp(ConnectView);
  mountedApp = app;
  app.use(router);
  app.use(pinia);
  const container = document.createElement('div');
  document.body.appendChild(container);
  app.mount(container);
  return { app, container };
}

async function submitUsernamePassword(
  container: HTMLElement,
  username: string,
  password: string,
) {
  // B003 期望的表单字段（红相：当前不存在 → null，赋空跳过，提交仍走旧 token 流 → FAIL）
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

describe('Connect.vue: username/password login form (B003 contract)', () => {
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

  it('表单渲染 username + password 双字段输入', async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    stubFetch(calls);
    const { container } = mountConnect(calls);
    // 挂载就绪：条件轮询直到表单渲染（确定性，替代硬编码 sleep）
    await waitFor(() => container.querySelector('form') !== null, '登录表单渲染');

    const userInput = container.querySelector('#username-input');
    const passInput = container.querySelector('#password-input');
    expect(userInput, '登录表单必须含 #username-input').not.toBeNull();
    expect(passInput, '登录表单必须含 #password-input').not.toBeNull();
  });

  it('提交 username/password → POST /api/user/login', async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    stubFetch(calls);
    const { container } = mountConnect(calls);
    // 挂载就绪：等待 onMounted 探针已执行（/v1/models）且表单渲染完成
    await waitFor(() => calls.some((c) => c.url === '/v1/models'), 'onMounted 探针完成');
    await waitFor(() => container.querySelector('form') !== null, '登录表单渲染');

    await submitUsernamePassword(container, 'alice', 'alice-pass-1234');

    // 提交结算：条件轮询直到 /api/user/login 调用出现，或错误信封渲染（红相回退路径）
    await waitFor(
      () =>
        calls.some((c) => c.url === '/api/user/login') ||
        container.querySelector('.error') !== null,
      '提交结算（login 调用或错误渲染）',
    );

    const loginCall = calls.find((c) => c.url === '/api/user/login');
    expect(
      loginCall,
      `登录必须 POST /api/user/login；实际调用: ${calls
        .map((c) => `${c.init?.method ?? 'GET'} ${c.url}`)
        .join(', ')}`,
    ).toBeDefined();
    expect(loginCall!.init?.method ?? 'GET').toBe('POST');
    expect(JSON.parse(String(loginCall!.init?.body))).toEqual({
      username: 'alice',
      password: 'alice-pass-1234',
    });
  });
});
