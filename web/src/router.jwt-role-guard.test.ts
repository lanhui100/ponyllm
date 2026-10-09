// @vitest-environment happy-dom
// B005 (W3/W4/W5) 红相契约测试：router 守卫的 JWT user-role 管理面门。
// 冻结契约：`.dev-team/contracts/wave-4-jwt-admin-bridge.md`：
// - W3: user JWT 直访 /dashboard → 跳 /tokens，且**不**触发 401「登录已过期」单飞
//   toast（守卫先于组件挂载重定向 → 不产生任何管理面请求 → alova 401 handler
//   不可能被调用）；
// - W4: admin JWT 直访 /dashboard → 停留（不跳转）；
// - W5: cookie/legacy 会话 → 不施加角色门（行为不变）。
// 红相主锚：当前 `router.ts` beforeEach 无 user-role JWT 守卫（decideRoute 仅 gate
// /users 的 requiresAdmin）→ user JWT 直入 /dashboard、DashboardView 挂载后撞管理面
// 401 单飞 → W3 FAIL（最终落 /connect 而非 /tokens、触发「登录已过期」toast、
// 发出管理面请求）。
// 测试形态：singleton router 挂真实 `<router-view>` app（不挂 app 则路由只确认不
// 实例化视图，401 单飞链路无法被真实驱动——曾致 W3 假超时）；先 push 到无副作用的
// catch-all 起始路由，避免 app 安装时的初始导航竞态。
// 防 Flaky 律（test-expert）：有界条件轮询结算，禁止硬编码 sleep。

import { beforeEach, describe, expect, it, afterEach, vi } from 'vitest';
import { createApp } from 'vue';
import { createPinia, setActivePinia } from 'pinia';
import { useSessionStore } from './stores/session';
import { router, setToastHandler, stopAllPolling } from './router';

const EXPIRED_TOAST = '登录已过期，请重新连接';
// 无副作用起始路由（catch-all → NotFound，纯模板零 fetch），避开初始导航竞态。
const START_PATH = '/__b005-guard-start__';

let mountedApp: ReturnType<typeof createApp> | null = null;

/// 有界条件轮询（防 Flaky）：每 20ms 复查，条件满足即返回；超时抛错（非盲目 sleep）。
async function waitFor(cond: () => boolean, what: string, timeoutMs = 3000): Promise<void> {
  const start = Date.now();
  while (!cond()) {
    if (Date.now() - start > timeoutMs) {
      throw new Error(`waitFor 超时（${timeoutMs}ms）：${what}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
}

/// 在 singleton router 上挂一个真实 `<router-view>` app：路由只 push 不挂 app 时，
/// 视图不会实例化（onMounted 不执行）→ 守卫后的挂载副作用（fetch/401 单飞）无法
/// 被真实驱动。此挂载使 W3 的"撞管理面 401 → 单飞 toast"链路可被物理断言。
function mountRouterApp(pinia: ReturnType<typeof createPinia>): void {
  const container = document.createElement('div');
  document.body.appendChild(container);
  const app = createApp({ template: '<router-view />' });
  mountedApp = app;
  app.use(router);
  app.use(pinia);
  app.mount(container);
}

/// W3 用户面 stub（建模当前+绿相后端）：
/// /health 200；__pony-probe__ 401；/api/user/** 200（用户面放行）；
/// /api/**（管理面）与 /v1/telemetry/** → 401（当前后端对 JWT 的既有行为）。
function stubUserJwtFetch(calls: { url: string; method?: string }[]) {
  vi.stubGlobal(
    'fetch',
    async (input: RequestInfo | URL, init?: RequestInit) => {
      const url =
        typeof input === 'string'
          ? input
          : input instanceof URL
            ? input.toString()
            : input.url;
      calls.push({ url, method: init?.method ?? 'GET' });
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
      const status = url.startsWith('/api/user/') ? 200 : 401;
      return new Response(JSON.stringify({}), {
        status,
        headers: { 'Content-Type': 'application/json' },
      });
    },
  );
}

describe('router guard: JWT user-role admin-plane gate (B005 W3/W4/W5)', () => {
  beforeEach(async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    stopAllPolling();
    window.sessionStorage.clear();
    // 确立无副作用起始路由（catch-all → NotFound）：guard 对无 requiresAuth 路由放行，
    // 且后续 app 安装因 currentRoute 已确认而不再触发初始导航（防竞态）。
    await router.push(START_PATH);
    mountRouterApp(pinia);
  });
  afterEach(() => {
    try {
      mountedApp?.unmount();
    } catch {
      /* noop */
    }
    mountedApp = null;
    setToastHandler(() => {});
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    document.body.innerHTML = '';
  });

  it('W3: user JWT 直访 /dashboard → 跳 /tokens 且不触发 401 单飞 toast（红相：当前直入并撞 401 → FAIL）', async () => {
    const toasts: string[] = [];
    setToastHandler((m) => toasts.push(m));
    const calls: { url: string; method?: string }[] = [];
    stubUserJwtFetch(calls);

    const session = useSessionStore();
    session.loginWithJwt('jwt-user-token', 'user');
    // 只统计登录后的请求：清空起始路由/挂载期的探针与协商调用
    calls.length = 0;
    await router.push('/dashboard');

    // 结算：绿相 → 守卫重定向至 /tokens（≠ /dashboard）；红相 → DashboardView 挂载后
    // 撞管理面 401 → 单飞 toast 触发 / 路由被弹到 /connect。有界轮询两者皆可结算。
    await waitFor(
      () => router.currentRoute.value.path !== '/dashboard' || toasts.length > 0,
      '守卫结算（跳 /tokens 或 401 单飞落地）',
      4000,
    );

    // 断言顺序：先管理面请求、再 toast、最后落点——expect.soft 让三项全红时同屏列出
    // 完整失败清单（不因首个断言失败而中断后续断言）。
    const adminCalls = calls.filter(
      (c) => c.url.startsWith('/api/admin/') || c.url.startsWith('/v1/telemetry/'),
    );
    expect.soft(
      adminCalls,
      'W3: user JWT 不得发出任何管理面请求（守卫先于组件挂载重定向 → alova 401 handler 不可达）',
    ).toEqual([]);
    expect.soft(
      toasts,
      'W3 严禁触发 401「登录已过期」单飞 toast（守卫重定向而非认证失败）',
    ).not.toContain(EXPIRED_TOAST);
    expect.soft(router.currentRoute.value.path, 'user JWT 直访 /dashboard 必须优雅落 /tokens（W3）').toBe('/tokens');
  }, 15000);

  it('W4: admin JWT 直访 /dashboard → 停留（回归守卫）', async () => {
    const toasts: string[] = [];
    setToastHandler((m) => toasts.push(m));
    const calls: { url: string; method?: string }[] = [];
    // admin JWT 管理面放行（绿相后端语义）：/api/** 与 /v1/telemetry/** → 200
    vi.stubGlobal('fetch', async (input: RequestInfo | URL, init?: RequestInit) => {
      const url =
        typeof input === 'string'
          ? input
          : input instanceof URL
            ? input.toString()
            : input.url;
      calls.push({ url, method: init?.method ?? 'GET' });
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

    const session = useSessionStore();
    session.loginWithJwt('jwt-admin-token', 'admin');
    calls.length = 0;
    await router.push('/dashboard');

    await waitFor(
      () => router.currentRoute.value.path === '/dashboard',
      'admin JWT 落 /dashboard',
      4000,
    );
    expect(router.currentRoute.value.path, 'admin JWT 直访 /dashboard 必须停留（W4）').toBe('/dashboard');
    expect(toasts, 'W4: admin JWT 停留不得触发任何 401 toast').toEqual([]);
  }, 15000);

  it('W5: cookie 会话访问 /dashboard → 停留（不施加角色门）', async () => {
    const toasts: string[] = [];
    setToastHandler((m) => toasts.push(m));
    const calls: { url: string; method?: string }[] = [];
    vi.stubGlobal('fetch', async (input: RequestInfo | URL, init?: RequestInit) => {
      const url =
        typeof input === 'string'
          ? input
          : input instanceof URL
            ? input.toString()
            : input.url;
      calls.push({ url, method: init?.method ?? 'GET' });
      return new Response(JSON.stringify({}), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      });
    });

    const session = useSessionStore();
    session.loginCookieMode('sid-1');
    calls.length = 0;
    await router.push('/dashboard');

    await waitFor(() => router.currentRoute.value.path === '/dashboard', 'cookie 会话落 /dashboard', 4000);
    expect(router.currentRoute.value.path, 'cookie 会话不得被角色门拦截（W5）').toBe('/dashboard');
    expect(toasts, 'W5: cookie 会话不得触发 401 toast').toEqual([]);
  }, 15000);

  it('W5: legacy 会话访问 /dashboard → 停留（不施加角色门；会话协商 404 回退 legacy）', async () => {
    const toasts: string[] = [];
    setToastHandler((m) => toasts.push(m));
    const calls: { url: string; method?: string }[] = [];
    // /api/admin/session → 404（协商回退 legacy）；其余 200
    vi.stubGlobal('fetch', async (input: RequestInfo | URL, init?: RequestInit) => {
      const url =
        typeof input === 'string'
          ? input
          : input instanceof URL
            ? input.toString()
            : input.url;
      calls.push({ url, method: init?.method ?? 'GET' });
      const status = url === '/api/admin/session' ? 404 : 200;
      return new Response(JSON.stringify({}), {
        status,
        headers: { 'Content-Type': 'application/json' },
      });
    });

    const session = useSessionStore();
    session.login('legacy-token');
    calls.length = 0;
    await router.push('/dashboard');

    await waitFor(() => router.currentRoute.value.path === '/dashboard', 'legacy 会话落 /dashboard', 4000);
    expect(router.currentRoute.value.path, 'legacy 会话不得被角色门拦截（W5）').toBe('/dashboard');
    expect(toasts, 'W5: legacy 会话不得触发 401 toast').toEqual([]);
  }, 15000);
});
