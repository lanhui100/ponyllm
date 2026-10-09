// @vitest-environment happy-dom
// B003 红相契约测试：Connect.vue 登录表单改 username/password 双字段 + 提交走
// /api/user/login（JWT 会话）。
// 红相：当前 Connect.vue 是单 token 输入（#token-input）→ 断言 #username-input /
// #password-input 存在、提交 POST /api/user/login → 预期 FAIL。

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { createApp, nextTick } from 'vue';
import { createPinia, setActivePinia } from 'pinia';
import { createRouter, createMemoryHistory } from 'vue-router';
import ConnectView from './Connect.vue';

const DASHBOARD = { template: '<div>dashboard</div>' };
let mountedApp: ReturnType<typeof createApp> | null = null;

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
  await new Promise((r) => setTimeout(r, 40));
}

describe('Connect.vue: username/password login form (B003 red)', () => {
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

  it('表单渲染 username + password 双字段输入（红相主锚：当前仅 #token-input → FAIL）', async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    stubFetch(calls);
    const { container } = mountConnect(calls);
    await new Promise((r) => setTimeout(r, 20));

    const userInput = container.querySelector('#username-input');
    const passInput = container.querySelector('#password-input');
    expect(userInput, '登录表单必须含 #username-input（B003 red）').not.toBeNull();
    expect(passInput, '登录表单必须含 #password-input（B003 red）').not.toBeNull();
  });

  it('提交 username/password → POST /api/user/login（红相：当前提交走 /api/admin/session / probe → FAIL）', async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    stubFetch(calls);
    const { container } = mountConnect(calls);
    await new Promise((r) => setTimeout(r, 20));

    await submitUsernamePassword(container, 'alice', 'alice-pass-1234');

    const loginCall = calls.find((c) => c.url === '/api/user/login');
    expect(
      loginCall,
      `登录必须 POST /api/user/login；实际调用: ${calls
        .map((c) => `${c.init?.method ?? 'GET'} ${c.url}`)
        .join(', ')}（B003 red）`,
    ).toBeDefined();
    expect(loginCall!.init?.method ?? 'GET').toBe('POST');
    expect(JSON.parse(String(loginCall!.init?.body))).toEqual({
      username: 'alice',
      password: 'alice-pass-1234',
    });
  });
});
