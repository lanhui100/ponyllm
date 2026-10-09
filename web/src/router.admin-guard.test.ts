// @vitest-environment happy-dom
// B003 红相契约测试：router 守卫的 admin 角色门（meta.requiresAdmin）。
// 红相：当前 decideRoute 无 role/requiresAdmin 参数、路由表无 /users 路由 → 断言预期 FAIL。
// 绿相契约（本文件以扩展签名固化）：普通用户角色访问 /users → 重定向 /connect；
// admin 角色 → 放行；未登录（无会话）→ 重定向。

import { beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useSessionStore } from './stores/session';
import { decideRoute, router } from './router';

// B003 期望的扩展签名（当前实现仅接受 5 参，多参在运行时被忽略 → 普通用户用例红失败）。
type DecideRouteExtended = (
  path: string,
  fullPath: string,
  requiresAuth: boolean,
  hasSession: boolean,
  role: string,
  requiresAdmin: boolean,
  doProbeOpenMode: () => Promise<boolean>,
) => Promise<true | { path: string; query: Record<string, string> }>;

const decideRouteExt = decideRoute as unknown as DecideRouteExtended;

describe('router guard: admin-role page gate (B003 red)', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    window.sessionStorage.clear();
  });

  it('普通用户角色访问 /users → 重定向 /connect（红相主锚：当前无 role 逻辑 → 放行 true → FAIL）', async () => {
    const verdict = await decideRouteExt('/users', '/users', true, true, 'user', true, async () => false);

    expect(verdict, '普通用户不得进入 requiresAdmin 页面（B003 red）').not.toBe(true);
    expect(verdict).toMatchObject({ path: '/connect' });
  });

  it('admin 角色访问 /users → 放行', async () => {
    const verdict = await decideRouteExt('/users', '/users', true, true, 'admin', true, async () => false);
    expect(verdict).toBe(true);
  });

  it('未登录访问 /users → 重定向 /connect（无论角色）', async () => {
    const verdict = await decideRouteExt('/users', '/users', true, false, 'user', true, async () => false);
    expect(verdict).toMatchObject({ path: '/connect' });
  });

  it('路由表声明 /users 且带 meta.requiresAdmin（红相：当前无此路由 → FAIL）', () => {
    const usersRoute = router.getRoutes().find((r) => r.path === '/users');
    expect(usersRoute, '路由表必须含 /users（B003 red）').toBeDefined();
    expect(usersRoute!.meta.requiresAdmin).toBe(true);
  });

  it('jwt 会话下的 hasSession 支持 admin 角色用户放行（session store 集成锚）', async () => {
    // 红相：store 无 jwt 模式 → role 未定义 → 无法按 admin 放行
    const session = useSessionStore();
    const view = session as unknown as Record<string, unknown>;
    expect(typeof view.loginWithJwt, 'loginWithJwt must exist (B003 red)').toBe('function');
    (view.loginWithJwt as (t: string, role?: string) => void)('jwt-x', 'admin');

    const verdict = await decideRouteExt('/users', '/users', true, session.hasSession(), 'admin', true, async () => false);
    expect(verdict).toBe(true);
  });
});
