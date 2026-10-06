import { createRouter, createWebHistory } from 'vue-router';
import type { RouteRecordRaw } from 'vue-router';
import { useSessionStore } from './stores/session';
import { setUnauthorizedHandler } from './lib/alova';
import ConnectView from './views/Connect.vue';
import NotFound from './views/NotFound.vue';

// WEB-02: Dashboard and Recorder views with dynamic chunk splitting
const routes: RouteRecordRaw[] = [
  { path: '/', redirect: '/dashboard' },
  { path: '/connect', component: ConnectView },
  { path: '/dashboard', component: () => import('./views/DashboardView.vue'), meta: { requiresAuth: true } },
  { path: '/recorder', component: () => import('./views/RecorderView.vue'), meta: { requiresAuth: true } },
  { path: '/governance', component: () => import('./views/GovernanceView.vue'), meta: { requiresAuth: true } },
  // Backward compatibility routes for legacy /app/* prefix
  { path: '/app', redirect: '/dashboard' },
  { path: '/app/dashboard', redirect: '/dashboard' },
  { path: '/app/recorder', redirect: '/recorder' },
  { path: '/app/governance', redirect: '/governance' },
  { path: '/app/connect', redirect: '/connect' },
  { path: '/:pathMatch(.*)*', component: NotFound },
];

export const router = createRouter({
  // Root path hosting (with backwards-compatible redirect for /app/*)
  history: createWebHistory('/'),
  routes,
});

// Polling stop registry: pages with polling (WEB-02) register their stop
// callback here; the 401 single-flight path invokes all of them once.
const stopPollingCallbacks = new Set<() => void>();

export function onStopPolling(callback: () => void): () => void {
  stopPollingCallbacks.add(callback);
  return () => {
    stopPollingCallbacks.delete(callback);
  };
}

export function stopAllPolling(): void {
  for (const stop of stopPollingCallbacks) {
    try {
      stop();
    } catch (error) {
      // One page's teardown must never block the others; warn for diagnosis.
      console.warn('[web] polling stop callback threw', error);
    }
  }
  stopPollingCallbacks.clear();
}

// Toast-once registry (same single-flight discipline as the redirect).
let toastHandler: ((message: string) => void) | null = null;

export function setToastHandler(handler: (message: string) => void): void {
  toastHandler = handler;
}

// No-auth probe endpoint: MUST be auth-covered GET (never /health —
// app.rs:21 exempts /health, so probing it always looks like open mode and
// silently bypasses authentication, P0-3 hardening).
export const PROBE_PATH = '/v1/models';

// Shared probe (single source; Connect.vue uses the same function, P2-2).
// Returns open-mode verdict; unreachable gateway resolves `true` so the guard
// never traps the user on /connect for a network failure — the page renders
// its own DOWN state (WEB-02). TODO(WEB-02): tri-state Open/Authed/Unknown.
export async function probeOpenMode(): Promise<boolean> {
  try {
    const resp = await fetch(PROBE_PATH, {
      headers: { Authorization: 'Bearer __pony-probe__' },
    });
    return resp.status !== 401;
  } catch {
    return true;
  }
}

export function extractFragmentToken(hash: string): string | null {
  if (!hash || !hash.startsWith('#')) return null;
  const raw = hash.slice(1);
  const params = new URLSearchParams(raw);
  const token = params.get('token') || params.get('key');
  if (token && token.trim() !== '') {
    return token.trim();
  }
  return null;
}

// In-memory transfer for fragment token so it never persists in URL / history
let memoryTransferToken: string | null = null;

export function consumeTransferToken(): string | null {
  const t = memoryTransferToken;
  memoryTransferToken = null;
  return t;
}

export function setTransferToken(token: string | null): void {
  memoryTransferToken = token;
}

export function sanitizeRedirectUrl(fullPath: string): string {
  if (!fullPath) return '/dashboard';
  try {
    const noHash = fullPath.split('#')[0];
    const [pathPart, queryPart] = noHash.split('?');
    const cleanPath = pathPart || '/dashboard';
    if (!queryPart) return cleanPath;
    const params = new URLSearchParams(queryPart);
    params.delete('token');
    params.delete('key');
    const qs = params.toString();
    return qs ? `${cleanPath}?${qs}` : cleanPath;
  } catch {
    return '/dashboard';
  }
}

router.beforeEach(async (to) => {
  const session = useSessionStore();

  // VULN-05 (Phase-3): 会话端点协商（幂等缓存）——GET /api/admin/session：
  // 200 → cookie 模式已登录；401(session_expired) → cookie 模式未登录；
  // 404 → legacy 回退（token 内存 / 旧部署兼容）。协商后统一走 hasSession()。
  if (session.sessionMode === 'unknown') {
    await session.negotiateSessionMode();
  }

  // 1. Support URL fragment/hash (#token= / #key=) to pass tokens securely:
  // Browser fragment is never sent to the server (not in access logs, referrer, or CDN cache).
  // Clean hash via Vue Router native replace to preserve router internal history.state and scroll.
  const hashToken = to.hash ? extractFragmentToken(to.hash) : null;
  if (hashToken) {
    setTransferToken(hashToken);
    const sanitizedRedirect = sanitizeRedirectUrl(to.fullPath);
    if (to.path === '/connect') {
      return { path: '/connect', query: to.query, hash: '', replace: true };
    }
    return {
      path: '/connect',
      query: { ...to.query, redirect: sanitizedRedirect },
      hash: '',
      replace: true,
    };
  }

  // 2. Deprecate and clean legacy query ?token= / ?key=:
  // Remove immediately from URL and redirect to /connect without exposing in logs.
  // Preserve any other legitimate business query parameters in the redirect target.
  const rawToken = (to.query.token || to.query.key) as string | undefined;
  if (rawToken && typeof rawToken === 'string' && rawToken.trim() !== '') {
    setTransferToken(rawToken.trim());
    const nextQuery = { ...to.query };
    delete nextQuery.token;
    delete nextQuery.key;
    const sanitizedRedirect = sanitizeRedirectUrl(to.fullPath);
    if (to.path === '/connect') {
      return { path: '/connect', query: nextQuery, replace: true };
    }
    return {
      path: '/connect',
      query: { ...nextQuery, redirect: sanitizedRedirect },
      replace: true,
    };
  }

  // Single-sourced on route meta (P1-2): adding a page with
  // `meta: { requiresAuth: true }` is automatically guarded; forgetting the
  // meta is the only way to bypass, and it is visible in the route table.
  const requiresAuth = to.matched.some((record) => record.meta.requiresAuth === true);
  return decideRoute(to.path, to.fullPath, requiresAuth, session.hasSession(), probeOpenMode);
});

// Pure guard decision (unit-testable without router singleton state).
export async function decideRoute(
  path: string,
  fullPath: string,
  requiresAuth: boolean,
  hasToken: boolean,
  doProbeOpenMode: () => Promise<boolean>,
): Promise<true | { path: string; query: Record<string, string> }> {
  if (path === '/connect') {
    return true;
  }
  if (!requiresAuth) {
    return true;
  }
  if (hasToken) {
    return true;
  }
  if (await doProbeOpenMode()) {
    return true;
  }
  return { path: '/connect', query: { redirect: fullPath } };
}

// 401 single-flight redirect: first claimer navigates + stops polling + toasts once.
setUnauthorizedHandler(() => {
  stopAllPolling();
  toastHandler?.('登录已过期，请重新连接');
  void router.push({ path: '/connect', query: { redirect: router.currentRoute.value.fullPath } });
});
