// @vitest-environment happy-dom
// 方案 B 红相契约测试：前端刷新恢复探活机制 (Session Restore Probe)
//
// 契约设计：
// 1. restoreSession / probeUserSession 探活机制：
//    - 应用初始化/页面刷新时，调用 probeUserSession (或 restoreSession)
//    - 在 cookie 模式下，浏览器自动附带 HttpOnly Cookie 访问 /api/user/me
//    - 若返回 200 及 user profile (role, username 等)，自动执行 session.loginWithJwt('', user.role) 或建立 loggedIn=true 态
//    - 若返回 401，保持 loggedIn=false 并清理状态
// 2. userApi fetch 调用凭据契约：
//    - 访问 /api/user/me 等端点时，fetch 必须携带 credentials: 'same-origin'（或 'include'），确保 HttpOnly Cookie 正常发送
// 3. 显式退出 logoutUser：
//    - POST /api/user/logout，服务端清除 HttpOnly Cookie，前端 session store 重置为未登录态

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useSessionStore } from './session';

describe('session restore & probe (Wave 1 red contract)', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.restoreAllMocks();
  });

  it('restoreUserSession探活：当 /api/user/me 返回有效用户时，自动恢复已登录态与角色', async () => {
    const store = useSessionStore();
    const view = store as unknown as Record<string, unknown>;

    // 模拟服务端响应：已登录用户 Alice (role: user)
    const mockUser = {
      id: 'usr-alice',
      username: 'alice',
      role: 'user',
      name: 'Alice',
      enabled: true,
      used_tokens: 0,
    };

    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = typeof input === 'string' ? input : (input as Request).url;
      if (url === '/api/user/me') {
        // 断言必须传递 same-origin 或 include 凭据配置以发送 HttpOnly Cookie
        expect(init?.credentials).toMatch(/same-origin|include/);
        return new Response(JSON.stringify(mockUser), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      return new Response('Not Found', { status: 404 });
    });

    // 契约：session store 暴露 restoreUserSession 方法（当前红相未实现，抛错/不存在 → FAIL）
    expect(
      typeof view.restoreUserSession,
      'restoreUserSession must exist on session store (Wave 1 red)',
    ).toBe('function');

    const restoreFn = view.restoreUserSession as () => Promise<boolean>;
    const restored = await restoreFn();

    expect(restored).toBe(true);
    expect(store.loggedIn).toBe(true);
    expect(store.hasSession()).toBe(true);
    expect(view.role).toBe('user');
    expect(view.sessionMode).toBe('jwt');

    expect(fetchSpy).toHaveBeenCalledWith(
      '/api/user/me',
      expect.objectContaining({ credentials: expect.stringMatching(/same-origin|include/) }),
    );
  });

  it('restoreUserSession探活：当 /api/user/me 返回 401 未登录时，不激活会话并保持未登录', async () => {
    const store = useSessionStore();
    const view = store as unknown as Record<string, unknown>;

    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
      const url = typeof input === 'string' ? input : (input as Request).url;
      if (url === '/api/user/me') {
        return new Response(JSON.stringify({ error: { message: 'not authenticated', code: 'invalid_token' } }), {
          status: 401,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      return new Response('Not Found', { status: 404 });
    });

    expect(
      typeof view.restoreUserSession,
      'restoreUserSession must exist on session store (Wave 1 red)',
    ).toBe('function');

    const restoreFn = view.restoreUserSession as () => Promise<boolean>;
    const restored = await restoreFn();

    expect(restored).toBe(false);
    expect(store.loggedIn).toBe(false);
    expect(store.hasSession()).toBe(false);
  });

  it('logoutUser 会话登出：POST /api/user/logout 清除 HttpOnly Cookie 并重置本地状态', async () => {
    const store = useSessionStore();
    const view = store as unknown as Record<string, unknown>;

    // 预置登录态
    store.loginWithJwt('dummy-jwt', 'admin');
    expect(store.loggedIn).toBe(true);

    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = typeof input === 'string' ? input : (input as Request).url;
      if (url === '/api/user/logout') {
        expect(init?.method).toBe('POST');
        expect(init?.credentials).toMatch(/same-origin|include/);
        return new Response(JSON.stringify({ ok: true }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      return new Response('Not Found', { status: 404 });
    });

    // 契约：logoutUser 方法支持调用后端 logout
    expect(
      typeof view.logoutUser,
      'logoutUser must exist on session store (Wave 1 red)',
    ).toBe('function');

    const logoutFn = view.logoutUser as () => Promise<void>;
    await logoutFn();

    expect(fetchSpy).toHaveBeenCalledWith(
      '/api/user/logout',
      expect.objectContaining({ method: 'POST', credentials: expect.stringMatching(/same-origin|include/) }),
    );
    expect(store.loggedIn).toBe(false);
    expect(store.hasSession()).toBe(false);
    expect(view.role).toBe('');
  });
});
