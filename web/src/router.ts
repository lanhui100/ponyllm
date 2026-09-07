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
  { path: '/:pathMatch(.*)*', component: NotFound },
];

export const router = createRouter({
  // Served under /app/* (vite base '/app/'): history base must match.
  history: createWebHistory('/app/'),
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

router.beforeEach(async (to) => {
  const session = useSessionStore();
  // Single-sourced on route meta (P1-2): adding a page with
  // `meta: { requiresAuth: true }` is automatically guarded; forgetting the
  // meta is the only way to bypass, and it is visible in the route table.
  const requiresAuth = to.matched.some((record) => record.meta.requiresAuth === true);
  return decideRoute(to.path, to.fullPath, requiresAuth, session.token !== '', probeOpenMode);
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
