// @vitest-environment happy-dom
// Phase-3 验收（VULN-05 前端 / task-9）：登录走 POST /api/admin/session（服务端
// 下发 HttpOnly cookie），前端不再以 Bearer 探测 /v1/models 后把 token 落 sessionStorage。
//
// 红相说明：
// 1. HEAD 上 Connect.vue submit() 以 Bearer 探测 /v1/models 并 `session.login(token)`
//    （客户端持久化）→ 无 /api/admin/session 调用（红相成立）；
// 2. HEAD 上 login → persistToken → 写 sessionStorage 'ponyllm_session_token'（红相成立）。
// 3. session_expired 401 信封 → 经 alova 401 单飞路径跳 /connect（companion：
//    HEAD 对任意 401 均已跳 /connect，该步为修复后的信封回归锚点）。

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { createApp, nextTick } from 'vue';
import { createPinia, setActivePinia } from 'pinia';
import { createRouter, createMemoryHistory } from 'vue-router';
import ConnectView from './Connect.vue';
import { SESSION_TOKEN_STORAGE_KEY } from '../stores/session';

const DASHBOARD = { template: '<div>dashboard</div>' };

let mountedApp: ReturnType<typeof createApp> | null = null;

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
    // 挂载期 openMode 探针（__pony-probe__）→ 401，令 `<form v-else>` 渲染；
    // 登录提交流（真实候选 token）→ 200。
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

async function submitForm(container: HTMLElement, token: string) {
  const input = container.querySelector('#token-input') as HTMLInputElement;
  input.value = token;
  input.dispatchEvent(new Event('input'));
  await nextTick();
  const form = container.querySelector('form') as HTMLFormElement;
  form.dispatchEvent(new Event('submit'));
  await new Promise((r) => setTimeout(r, 80));
}

describe('VULN-05 login flow', () => {
  beforeEach(() => {
    window.sessionStorage?.clear();
  });
  afterEach(() => {
    // 断言失败也要卸载，避免跨用例泄漏（红相阶段失败是预期的）
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

  it('login submits credentials to POST /api/admin/session (not a /v1/models probe)', async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    stubFetch(calls);
    const { container } = mountConnect(calls);
    await new Promise((r) => setTimeout(r, 20));
    await submitForm(container, 'sk-pony-admin-123');
    await new Promise((r) => setTimeout(r, 20));

    const loginCall = calls.find((c) => c.url.includes('/api/admin/session'));
    expect(
      loginCall,
      `登录必须 POST /api/admin/session；实际 fetch 调用: ${calls
        .map((c) => `${c.init?.method ?? 'GET'} ${c.url}`)
        .join(', ')}（HEAD 上探测 /v1/models → 红相成立）`
    ).toBeDefined();
    expect(loginCall!.init?.method ?? 'GET').toBe('POST');
  });

  it('successful login never writes the bearer token to sessionStorage', async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    stubFetch(calls);
    const { container } = mountConnect(calls);
    await new Promise((r) => setTimeout(r, 20));
    await submitForm(container, 'sk-pony-admin-123');
    await new Promise((r) => setTimeout(r, 20));

    // 内容断言（不依赖原型 spy）：登录成功后 token 不得落 sessionStorage
    expect(
      window.sessionStorage.getItem(SESSION_TOKEN_STORAGE_KEY),
      '登录成功不得写 sessionStorage(ponyllm_session_token)（HEAD 上 persistToken 写入 → 红相成立）'
    ).toBeNull();
  });
});