// @vitest-environment happy-dom
// B003 红相契约测试：session store 的 jwt 模式（VULN-05 纪律：JWT 仅内存，绝不落
// localStorage/sessionStorage；SESSION_TOKEN_STORAGE_KEY 永不被读写）。
// 红相：当前 session.ts 无 loginWithJwt / jwt 模式 / role 状态 → 断言预期 FAIL。

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useSessionStore, SESSION_TOKEN_STORAGE_KEY } from './session';

type StoreView = Record<string, unknown>;

describe('session store: B003 jwt mode (red)', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    window.sessionStorage.clear();
    vi.restoreAllMocks();
  });

  it('loginWithJwt exists and stores the token in memory with loggedIn=true', () => {
    const store = useSessionStore();
    const view = store as unknown as StoreView;

    // 契约：新增 loginWithJwt(token)（红相：当前不存在 → FAIL）
    expect(typeof view.loginWithJwt, 'loginWithJwt must exist (B003 red)').toBe('function');
    (view.loginWithJwt as (t: string) => void)('jwt-token-abc');

    // JWT 仅内存：token 入 ref、loggedIn 成立、hasSession 成立
    expect(store.loggedIn).toBe(true);
    expect(store.hasSession()).toBe(true);
    expect(view.token).toBe('jwt-token-abc');
    expect(view.sessionMode).toBe('jwt');
  });

  it('loginWithJwt stores role and logout clears the jwt session', () => {
    const store = useSessionStore();
    const view = store as unknown as StoreView;

    expect(typeof view.loginWithJwt, 'loginWithJwt must exist (B003 red)').toBe('function');
    (view.loginWithJwt as (t: string, role?: string) => void)('jwt-token-abc', 'admin');

    // 角色状态随登录写入（红相：当前无 role 状态 → undefined ≠ admin → FAIL）
    expect(view.role).toBe('admin');

    store.logout();
    expect(store.loggedIn).toBe(false);
    expect(store.hasSession()).toBe(false);
    expect(view.token).toBe('');
  });

  it('jwt login NEVER reads or writes the bearer-token storage key (VULN-05)', () => {
    const getSpy = vi.spyOn(Storage.prototype, 'getItem');
    const setSpy = vi.spyOn(Storage.prototype, 'setItem');
    const removeSpy = vi.spyOn(Storage.prototype, 'removeItem');

    const store = useSessionStore();
    const view = store as unknown as StoreView;
    // 直接调用（不用可选链）：loginWithJwt 缺失时抛 TypeError → 本用例红失败，绝不空转
    (view.loginWithJwt as (t: string) => void)('jwt-token-abc');
    store.logout();

    const isTokenKey = (key: unknown) => key === SESSION_TOKEN_STORAGE_KEY;
    expect(
      getSpy.mock.calls.filter((c) => isTokenKey(c[0])),
      'JWT 登录/登出不得 getItem(ponyllm_session_token)（VULN-05 纪律，红相锚）',
    ).toHaveLength(0);
    expect(
      setSpy.mock.calls.filter((c) => isTokenKey(c[0])),
      'JWT 登录/登出不得 setItem(ponyllm_session_token)',
    ).toHaveLength(0);
    expect(
      removeSpy.mock.calls.filter((c) => isTokenKey(c[0])),
      'JWT 登录/登出不得 removeItem(ponyllm_session_token)',
    ).toHaveLength(0);
  });
});
