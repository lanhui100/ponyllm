// @vitest-environment happy-dom
// Phase-3 验收（VULN-05 前端 / task-9）：会话凭据不再经 sessionStorage 持久化。
//
// 红相说明：HEAD 上 session.ts 的 `getInitialToken` 读取
// `ponyllm_session_token`、`persistToken` 写入/删除该键（Bearer token 落在
// 客户端可读存储，配合 XSS 即会话泄露）。修复后该键不得有任何读写 → 本文件断言
// 全为空（红相成立）。

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useSessionStore, SESSION_TOKEN_STORAGE_KEY } from './session';

describe('VULN-05 session store: no bearer-token sessionStorage persistence', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    window.sessionStorage.clear();
  });

  it('never reads or writes the bearer-token storage key', () => {
    const getSpy = vi.spyOn(Storage.prototype, 'getItem');
    const setSpy = vi.spyOn(Storage.prototype, 'setItem');
    const removeSpy = vi.spyOn(Storage.prototype, 'removeItem');

    // 触发 store 初始化（getInitialToken）与 login/logout/clearToken（persistToken）
    const session = useSessionStore();
    session.login('sk-pony-admin-abc');
    session.logout();
    session.login('sk-pony-admin-abc');
    session.clearToken();

    const isTokenKey = (key: unknown) => key === SESSION_TOKEN_STORAGE_KEY;
    expect(
      getSpy.mock.calls.filter((c) => isTokenKey(c[0])),
      '不得 getItem(ponyllm_session_token) (HEAD 上 getInitialToken 读取 → 红相成立)'
    ).toHaveLength(0);
    expect(
      setSpy.mock.calls.filter((c) => isTokenKey(c[0])),
      '不得 setItem(ponyllm_session_token) (HEAD 上 persistToken 写入 → 红相成立)'
    ).toHaveLength(0);
    expect(
      removeSpy.mock.calls.filter((c) => isTokenKey(c[0])),
      '不得 removeItem(ponyllm_session_token)'
    ).toHaveLength(0);
  });
});
describe('VULN-05 / R-S4: logout revokes the server session in cookie mode', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    window.sessionStorage.clear();
  });

  it('logout issues POST /api/admin/session/revoke (cookie mode)', async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    vi.stubGlobal(
      'fetch',
      async (input: RequestInfo | URL, init?: RequestInit) => {
        calls.push({ url: String(input), init });
        return new Response(null, { status: 204 });
      },
    );

    const session = useSessionStore();
    session.loginCookieMode();
    session.logout();

    const revoke = calls.find((c) => c.url.includes('/api/admin/session/revoke'));
    expect(
      revoke,
      'logout 必须调用 POST /api/admin/session/revoke（HEAD 上 logout 纯客户端清内存 → 红相成立）'
    ).toBeDefined();
    expect(revoke!.init?.method ?? 'GET').toBe('POST');
  });
});
