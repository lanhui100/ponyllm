// @vitest-environment happy-dom
// B003 红相契约测试：userApi 封装（/api/user/login + /api/user/tokens CRUD/rotate）。
// 红相：userApi.ts 目前是"仅契约面最小桩"（函数 throw B003 not implemented）→ 断言预期 FAIL。
// 绿相：实现替换后这些断言必须全绿（fetch 路径、请求体、错误信封、返回形态）。

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  loginWithPassword,
  listMyTokens,
  createMyToken,
  updateMyToken,
  deleteMyToken,
  rotateMyToken,
  type LoginResponse,
  type UserTokenMeta,
} from './userApi';

describe('userApi: /api/user/login (B003 red)', () => {
  beforeEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it('loginWithPassword POSTs /api/user/login with {username,password} and returns access_token', async () => {
    // Arrange —— 捕获 fetch 调用并按契约返回 200 信封
    const calls: { url: string; init?: RequestInit }[] = [];
    vi.stubGlobal('fetch', async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = typeof input === 'string' ? input : input.toString();
      calls.push({ url, init });
      return new Response(
        JSON.stringify({
          access_token: 'jwt-token-abc',
          user: { id: 'usr-alice', username: 'alice', role: 'user', name: 'Alice', enabled: true },
        } satisfies LoginResponse),
        { status: 200, headers: { 'Content-Type': 'application/json' } },
      );
    });

    // Act
    const resp = await loginWithPassword('alice', 'alice-pass-1234');

    // Assert
    expect(
      calls.find((c) => c.url === '/api/user/login'),
      'loginWithPassword 必须 POST /api/user/login（红相：桩未实现 → 无调用 → FAIL）',
    ).toBeDefined();
    const loginCall = calls.find((c) => c.url === '/api/user/login')!;
    expect(loginCall.init?.method ?? 'GET').toBe('POST');
    expect(loginCall.init?.headers).toMatchObject({ 'Content-Type': 'application/json' });
    expect(JSON.parse(String(loginCall.init?.body))).toEqual({
      username: 'alice',
      password: 'alice-pass-1234',
    });
    expect(resp.access_token).toBeTypeOf('string');
    expect(resp.user.username).toBe('alice');
  });

  it('login failure surfaces the invalid_credentials envelope as a rejection', async () => {
    vi.stubGlobal('fetch', async () =>
      new Response(JSON.stringify({ error: { code: 'invalid_credentials', message: 'bad' } }), {
        status: 401,
        headers: { 'Content-Type': 'application/json' },
      }),
    );

    await expect(loginWithPassword('alice', 'WRONG')).rejects.toMatchObject({
      code: 'invalid_credentials',
    });
  });
});

describe('userApi: /api/user/tokens CRUD/rotate (B003 red)', () => {
  beforeEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it('listMyTokens GETs /api/user/tokens and returns rows with used_tokens', async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    vi.stubGlobal('fetch', async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = typeof input === 'string' ? input : input.toString();
      calls.push({ url, init });
      return new Response(
        JSON.stringify([
          { key_id: 'tk-1', name: 'ci', model_limits: null, quota: 1000, expires_at: null, enabled: true, used_tokens: 7 },
        ] satisfies UserTokenMeta[]),
        { status: 200, headers: { 'Content-Type': 'application/json' } },
      );
    });

    const rows = await listMyTokens();

    expect(
      calls.find((c) => c.url === '/api/user/tokens'),
      'listMyTokens 必须 GET /api/user/tokens（红相：桩未实现 → FAIL）',
    ).toBeDefined();
    expect(calls.find((c) => c.url === '/api/user/tokens')!.init?.method ?? 'GET').toBe('GET');
    expect(rows).toHaveLength(1);
    expect(rows[0].key_id).toBe('tk-1');
    expect(rows[0].used_tokens).toBe(7);
  });

  it('createMyToken POSTs and returns one-time plaintext api_key', async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    vi.stubGlobal('fetch', async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = typeof input === 'string' ? input : input.toString();
      calls.push({ url, init });
      return new Response(
        JSON.stringify({ key_id: 'tk-new', api_key: 'sk-pony-abcdef', name: 'ci', model_limits: null, quota: null, expires_at: null, enabled: true, used_tokens: 0 }),
        { status: 201, headers: { 'Content-Type': 'application/json' } },
      );
    });

    const created = await createMyToken({ name: 'ci-token', model_limits: ['gpt-4o-mini'], quota: 1000 });

    const createCall = calls.find((c) => c.url === '/api/user/tokens')!;
    expect(createCall.init?.method ?? 'GET').toBe('POST');
    expect(JSON.parse(String(createCall.init?.body))).toEqual({
      name: 'ci-token',
      model_limits: ['gpt-4o-mini'],
      quota: 1000,
    });
    expect(created.api_key).toBe('sk-pony-abcdef');
    expect(created.used_tokens).toBe(0);
  });

  it('updateMyToken PUTs /api/user/tokens/{key_id} with patch body', async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    vi.stubGlobal('fetch', async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = typeof input === 'string' ? input : input.toString();
      calls.push({ url, init });
      return new Response(
        JSON.stringify({ key_id: 'tk-1', name: 'renamed', model_limits: null, quota: 500, expires_at: null, enabled: false, used_tokens: 7 }),
        { status: 200, headers: { 'Content-Type': 'application/json' } },
      );
    });

    const updated = await updateMyToken('tk-1', { name: 'renamed', quota: 500, enabled: false });

    const call = calls.find((c) => c.url === '/api/user/tokens/tk-1')!;
    expect(call.init?.method ?? 'GET').toBe('PUT');
    expect(JSON.parse(String(call.init?.body))).toEqual({ name: 'renamed', quota: 500, enabled: false });
    expect(updated.name).toBe('renamed');
    expect(updated.enabled).toBe(false);
  });

  it('deleteMyToken DELETEs /api/user/tokens/{key_id}', async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    vi.stubGlobal('fetch', async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = typeof input === 'string' ? input : input.toString();
      calls.push({ url, init });
      return new Response(null, { status: 200 });
    });

    await deleteMyToken('tk-1');

    const call = calls.find((c) => c.url === '/api/user/tokens/tk-1')!;
    expect(call.init?.method ?? 'GET').toBe('DELETE');
  });

  it('rotateMyToken POSTs rotate and returns new plaintext', async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    vi.stubGlobal('fetch', async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = typeof input === 'string' ? input : input.toString();
      calls.push({ url, init });
      return new Response(JSON.stringify({ api_key: 'sk-pony-rotated' }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      });
    });

    const rotated = await rotateMyToken('tk-1');

    const call = calls.find((c) => c.url === '/api/user/tokens/tk-1/rotate')!;
    expect(call.init?.method ?? 'GET').toBe('POST');
    expect(rotated.api_key).toBe('sk-pony-rotated');
  });
});
