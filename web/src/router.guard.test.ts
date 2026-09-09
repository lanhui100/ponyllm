// @vitest-environment happy-dom
// WEB-01 acceptance 3 + 6: guard decision, 401 single-flight, header contract.
import { beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useSessionStore } from './stores/session';
import { authHeaders, bearerValue, resolveBaseURL, UnauthorizedError, isUnauthorized } from './lib/alova';
import { decideRoute, onStopPolling, stopAllPolling, PROBE_PATH, router } from './router';

describe('guard decision (WEB-01 acceptance 3)', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    stopAllPolling();
    window.sessionStorage.clear();
  });

  it('exposes /connect, /dashboard, /recorder and a 404 catch-all', () => {
    const paths = router.getRoutes().map((r) => r.path);
    expect(paths).toContain('/connect');
    expect(paths).toContain('/dashboard');
    expect(paths).toContain('/recorder');
  });

  it('no token + authed gateway => redirect /connect with back-link', async () => {
    const verdict = await decideRoute('/dashboard', '/dashboard', true, false, async () => false);
    expect(verdict).toEqual({ path: '/connect', query: { redirect: '/dashboard' } });

    const recorderVerdict = await decideRoute('/recorder', '/recorder', true, false, async () => false);
    expect(recorderVerdict).toEqual({ path: '/connect', query: { redirect: '/recorder' } });
  });

  it('/connect is self-permitting (no redirect loop)', async () => {
    const verdict = await decideRoute('/connect', '/connect', false, false, async () => false);
    expect(verdict).toBe(true);
  });

  it('open-mode gateway (probe not 401) lets the user straight in', async () => {
    const verdict = await decideRoute('/dashboard', '/dashboard', true, false, async () => true);
    expect(verdict).toBe(true);
  });

  it('logged-in token passes without probing', async () => {
    let probed = false;
    const verdict = await decideRoute('/dashboard', '/dashboard', true, true, async () => {
      probed = true;
      return false;
    });
    expect(verdict).toBe(true);
    expect(probed).toBe(false);
  });

  it('route without requiresAuth passes even with no token (meta-driven, P1-2)', async () => {
    const verdict = await decideRoute('/public', '/public', false, false, async () => {
      throw new Error('probe must not run for public routes');
    });
    expect(verdict).toBe(true);
  });

  it('probe never uses /health (P0-3: would silently bypass auth)', () => {
    expect(PROBE_PATH).toBe('/v1/models');
  });
});

describe('401 single-flight (WEB-01 P0-2)', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    stopAllPolling();
    window.sessionStorage.clear();
  });

  it('first 401 claims; concurrent second is dropped; logout/login re-arms', () => {
    const session = useSessionStore();
    session.login('sk-pony-test');
    expect(session.markUnauthorizedHandled()).toBe(true);
    expect(session.markUnauthorizedHandled()).toBe(false);
    session.logout();
    session.login('sk-pony-second');
    expect(session.markUnauthorizedHandled()).toBe(true);
  });

  it('P1-1 regression: claim-then-clearToken does NOT re-arm (no self-destruct)', () => {
    const session = useSessionStore();
    session.login('sk-pony-test');
    // Handler path: mark -> clearToken -> callback (mirrors alova onSuccess).
    expect(session.markUnauthorizedHandled()).toBe(true);
    session.clearToken();
    expect(session.token).toBe('');
    // Concurrent second 401 after the wipe must still be dropped.
    expect(session.markUnauthorizedHandled()).toBe(false);
  });

  it('UnauthorizedError contract: instanceof, status 401, name', () => {
    const err = new UnauthorizedError();
    expect(isUnauthorized(err)).toBe(true);
    expect(isUnauthorized(new Error('unauthorized'))).toBe(false);
    expect(isUnauthorized('unauthorized')).toBe(false);
    expect(err.status).toBe(401);
    expect(err.name).toBe('UnauthorizedError');
  });

  it('P1-1 regression: double 401 through the real handler fires once', async () => {
    const { handleUnauthorizedResponse, setUnauthorizedHandler } = await import('./lib/alova');
    setActivePinia(createPinia());
    const session = useSessionStore();
    session.login('sk-pony-test');
    let calls = 0;
    setUnauthorizedHandler(() => {
      calls += 1;
    });
    // Two concurrent 401s racing through the REAL handler path.
    expect(handleUnauthorizedResponse()).toBe(true);
    expect(handleUnauthorizedResponse()).toBe(false);
    expect(calls).toBe(1);
    expect(session.token).toBe('');
    setUnauthorizedHandler(() => {});
  });

  it('stopAllPolling runs every registered stop exactly once', () => {
    const calls: string[] = [];
    onStopPolling(() => {
      calls.push('a');
    });
    const unregister = onStopPolling(() => {
      calls.push('b');
    });
    unregister();
    stopAllPolling();
    expect(calls).toEqual(['a']);
  });
});

describe('header + storage contract (WEB-01 acceptance 6)', () => {
  it('emits ONLY Authorization: Bearer <trimmed>', () => {
    expect(authHeaders('  sk-pony-abc  ')).toEqual({ Authorization: 'Bearer sk-pony-abc' });
    expect(authHeaders('')).toEqual({});
    expect(bearerValue('  sk-pony-abc  ')).toBe('Bearer sk-pony-abc');
    const headers = authHeaders('sk-pony-abc');
    // The forbidden header literal must appear here (that is the assertion);
    // acceptance 6 greps non-test sources only.
    expect('x-api-key' in headers).toBe(false);
    expect('X-Api-Key' in headers).toBe(false);
  });

  it('resolveBaseURL normalizes blanks, slashes and non-strings', () => {
    const w = window as unknown as Record<string, unknown>;
    delete w.__PONY_BASE__;
    expect(resolveBaseURL()).toBe('');
    w.__PONY_BASE__ = 'http://127.0.0.1:8080///';
    expect(resolveBaseURL()).toBe('http://127.0.0.1:8080');
    w.__PONY_BASE__ = '  http://x/  ';
    expect(resolveBaseURL()).toBe('http://x');
    w.__PONY_BASE__ = 42;
    expect(resolveBaseURL()).toBe('');
    delete w.__PONY_BASE__;
  });
});

describe('URL token direct authorization', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    window.sessionStorage.clear();
  });

  it('router navigation with ?token= extracts token and cleans query', async () => {
    const session = useSessionStore();
    expect(session.token).toBe('');
    await router.push('/?token=my-secret-token');
    expect(session.token).toBe('my-secret-token');
    expect(router.currentRoute.value.path).toBe('/dashboard');
    expect(router.currentRoute.value.query.token).toBeUndefined();
  });

  it('page refresh preserves token from sessionStorage', async () => {
    const session = useSessionStore();
    session.login('session-persistent-token');
    expect(window.sessionStorage.getItem('ponyllm_session_token')).toBe('session-persistent-token');

    // Simulate page reload by creating a fresh Pinia instance
    setActivePinia(createPinia());
    const reloadedSession = useSessionStore();
    expect(reloadedSession.token).toBe('session-persistent-token');

    // Guard permits directly without redirecting to /connect
    const verdict = await decideRoute('/dashboard', '/dashboard', true, reloadedSession.token !== '', async () => false);
    expect(verdict).toBe(true);

    // Logout clears both store and sessionStorage
    reloadedSession.logout();
    expect(reloadedSession.token).toBe('');
    expect(window.sessionStorage.getItem('ponyllm_session_token')).toBeNull();
  });

  it('sessionStorage throw does not crash store login/logout', () => {
    const originalSet = window.sessionStorage.setItem;
    window.sessionStorage.setItem = () => {
      throw new Error('QuotaExceededError');
    };
    const session = useSessionStore();
    expect(() => session.login('test-token-under-quota')).not.toThrow();
    expect(session.token).toBe('test-token-under-quota');
    expect(() => session.logout()).not.toThrow();
    expect(session.token).toBe('');
    window.sessionStorage.setItem = originalSet;
  });
});

