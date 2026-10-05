// @vitest-environment happy-dom
// Phase-2 安全修复验收（F14 / VULN-18 OAuth noopener + 失效 postMessage 通道移除）。
//
// 红相要求（HEAD 均失败 = 未实现证据）：
// 1. `fetchAndOpenAuthUrl` 用 `noopener=no` 打开弹窗 —— window.open features 里
//    `noopener=no` 明确表示"不启用 noopener"，弹窗保留 opener 引用（可被反向 window.opener
//    攻击）。修复后必须 `noopener=yes`。
// 2. 组件注册 `window.addEventListener('message', handleWindowMessage)` 失效 postMessage
//    通道（回调 HTML 的 postMessage 在新 OAuth 流程中已不再使用）。修复后须移除该监听。

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { createApp, nextTick } from 'vue';
import { createPinia, setActivePinia } from 'pinia';
import { createRouter, createMemoryHistory } from 'vue-router';
import GovernanceView from './GovernanceView.vue';
import { useSessionStore } from '../stores/session';
import { adminApi } from '../lib/adminApi';

const mockOverviewWritable = {
  version: '0.2.26',
  bind: '127.0.0.1:8080',
  auth_mode: 'token',
  providers: 1,
  keys: 1,
  keys_active: 1,
  strategy: 'economy',
  hot_reload_ms: 1000,
  admin_write_enabled: true,
  config_version: 10,
};

const mockProviders: unknown[] = [];
const mockModels: unknown[] = [];
const mockKeys: unknown[] = [];
const mockStrategy = { strategy: 'economy', config_version: 10 };

const mockAuthUrl = {
  auth_url: 'https://accounts.google.com/o/oauth2/v2/auth?client_id=dummy',
  redirect_uri: 'http://localhost:51121/oauth2callback',
  state: 'state-f14',
};

describe('F14 OAuth popup noopener + dead postMessage channel removed', () => {
  // 原始引用在文件级捕获，afterEach 无条件恢复 —— 红相阶段断言失败是预期，绝不能污染后续用例
  const origOpen = window.open;
  const origAddEventListener = window.addEventListener;
  let pinia: ReturnType<typeof createPinia>;
  let container: HTMLDivElement;

  beforeEach(() => {
    window.sessionStorage?.clear();
    pinia = createPinia();
    setActivePinia(pinia);
    container = document.createElement('div');
    document.body.appendChild(container);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    (window as any).open = origOpen;
    (window as any).addEventListener = origAddEventListener;
    if (container.parentNode) document.body.removeChild(container);
  });

  async function mountAndTriggerOAuth() {
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [{ path: '/governance', component: GovernanceView }],
    });
    const session = useSessionStore(pinia);
    session.login('sk-admin-token');
    vi.spyOn(adminApi, 'getOverview').mockReturnValue({ send: () => Promise.resolve(mockOverviewWritable) } as any);
    vi.spyOn(adminApi, 'getProviders').mockReturnValue({ send: () => Promise.resolve(mockProviders) } as any);
    vi.spyOn(adminApi, 'getModels').mockReturnValue({ send: () => Promise.resolve(mockModels) } as any);
    vi.spyOn(adminApi, 'getKeys').mockReturnValue({ send: () => Promise.resolve(mockKeys) } as any);
    vi.spyOn(adminApi, 'getStrategy').mockReturnValue({ send: () => Promise.resolve(mockStrategy) } as any);
    vi.spyOn(adminApi, 'getAntigravityAuthUrl').mockReturnValue({
      send: () => Promise.resolve(mockAuthUrl),
    } as any);

    const app = createApp(GovernanceView);
    app.use(router);
    app.use(pinia);
    app.mount(container);
    await nextTick();
    await new Promise((r) => setTimeout(r, 50));

    // 打开新增 Provider 面板 → 切到 Antigravity 模式 → 点击"获取授权链接并打开弹窗"
    // 每步留出 50ms：模式切换含 async loadAntigravityAuthUrl（adminApi mock 链）
    // 与 Vue 响应式 flush，等待不足会取到未就绪的按钮/未触发的异步流。
    const addBtn = container.querySelector('[data-testid="add-provider-btn"]') as HTMLButtonElement;
    addBtn.click();
    await new Promise((r) => setTimeout(r, 50));
    const agBtn = container.querySelector('[data-testid="mode-antigravity-btn"]') as HTMLButtonElement;
    agBtn.click();
    await new Promise((r) => setTimeout(r, 50));
    const fetchBtn = container.querySelector('[data-testid="ag-fetch-url-btn"]') as HTMLButtonElement;
    fetchBtn.click();
    await new Promise((r) => setTimeout(r, 200));

    return app;
  }

  it('opens the OAuth popup with noopener=yes (never noopener=no)', async () => {
    const openCalls: string[] = [];
    (window as any).open = ((_url: string, _name: string, features?: string) => {
      openCalls.push(features ?? '');
      return null;
    }) as typeof window.open;

    const app = await mountAndTriggerOAuth();
    app.unmount();

    expect(openCalls.length).toBeGreaterThan(0);
    const features = openCalls[0];
    expect(features, `window.open features: ${features}`).toContain('noopener=yes');
    expect(features, `window.open features: ${features}`).not.toContain('noopener=no');
  });

  it('no longer registers a window "message" listener (dead postMessage channel removed)', async () => {
    const addedTypes: string[] = [];
    (window as any).addEventListener = ((type: string, listener: EventListenerOrEventListenerObject, opts?: unknown) => {
      addedTypes.push(type);
      return origAddEventListener.call(window, type, listener, opts as AddEventListenerOptions);
    }) as typeof window.addEventListener;

    const app = await mountAndTriggerOAuth();
    app.unmount();

    const messageRegistrations = addedTypes.filter((t) => t === 'message');
    expect(
      messageRegistrations,
      `注册的 message 监听次数：${addedTypes.join(', ')}（HEAD 上 fetchAndOpenAuthUrl 注册 postMessage 通道 → 1，红相成立）`
    ).toHaveLength(0);
  });
});
