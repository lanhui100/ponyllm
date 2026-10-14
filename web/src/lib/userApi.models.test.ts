// @vitest-environment happy-dom
// wave-6 红相契约测试：userApi.listAvailableModels()（GET /api/user/models）。
//
// 冻结契约：.dev-team/contracts/wave-6-user-models-picker.md §2.2（前端实现期行为契约）。
// 红相基线（T1 空桩 `throw new Error('NotImplemented')`）：本文件全部用例必须真实 FAIL
// （fetch 从未发出 → 调用记录断言失败；非编译错误）。
// 绿相（task-4 实现后）：本文件必须全绿，且不得修改本文件。
//
// 注：契约 §3 的 T-F1（断言空桩抛 NotImplemented）是"空桩期基线"，实现后必然无法为绿，
// 因此不作为冻结断言写入本文件；红相有效性由"桩不发 fetch"本身承载。

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { listAvailableModels } from './userApi';
import { useSessionStore } from '../stores/session';

interface FetchCall {
  url: string;
  init?: RequestInit;
}

/** 安装 fetch stub，记录全部调用并按给定 (status, body) 应答。 */
function stubFetch(status: number, body: unknown, raw?: string): FetchCall[] {
  const calls: FetchCall[] = [];
  vi.stubGlobal('fetch', async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input.toString();
    calls.push({ url, init });
    return new Response(raw ?? JSON.stringify(body), {
      status,
      headers: { 'Content-Type': 'application/json' },
    });
  });
  return calls;
}

/** 捕获调用结果：返回 { value } 或 { error }，避免吞异常式放行。 */
async function capture<T>(fn: () => Promise<T>): Promise<{ value?: T; error?: unknown }> {
  try {
    return { value: await fn() };
  } catch (error) {
    return { error };
  }
}

describe('userApi.listAvailableModels (wave-6 红相)', () => {
  beforeEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    setActivePinia(createPinia());
    useSessionStore().token = 'test-jwt-token';
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it('GETs /api/user/models with Bearer JWT + same-origin and parses body.models in order', async () => {
    // Arrange
    const payload = {
      models: [
        { id: 'auto', provider: 'ponyllm', display_name: 'Auto(智能·高可用主力)' },
        { id: 'gpt-4o', provider: 'openai', display_name: null },
        { id: 'gpt-4o-mini', provider: 'openai', display_name: 'GPT-4o mini' },
      ],
    };
    const calls = stubFetch(200, payload);

    // Act
    const result = await listAvailableModels();

    // Assert
    expect(
      calls.length,
      'listAvailableModels 必须发出真实 fetch（红相：空桩 throw NotImplemented → 0 次调用 → FAIL）',
    ).toBe(1);
    expect(calls[0].url).toBe('/api/user/models');
    expect(calls[0].init?.method ?? 'GET').toBe('GET');
    expect(calls[0].init?.headers).toMatchObject({
      Authorization: 'Bearer test-jwt-token',
    });
    expect(calls[0].init?.credentials).toBe('same-origin');
    expect(result).toEqual([
      { id: 'auto', provider: 'ponyllm', display_name: 'Auto(智能·高可用主力)' },
      { id: 'gpt-4o', provider: 'openai', display_name: null },
      { id: 'gpt-4o-mini', provider: 'openai', display_name: 'GPT-4o mini' },
    ]);
    expect(result.map((m) => m.id)).toEqual(['auto', 'gpt-4o', 'gpt-4o-mini']);
  });

  it('returns [] for 200 with empty models array', async () => {
    // Arrange
    stubFetch(200, { models: [] });

    // Act
    const result = await listAvailableModels();

    // Assert
    expect(result).toEqual([]);
  });

  it.each([
    ['missing models key', {}, undefined],
    ['models = null', { models: null }, undefined],
    ['models = string', { models: 'not-an-array' }, undefined],
    ['models = object', { models: { id: 'x' } }, undefined],
    ['body is not JSON', undefined, '<<not json>>'],
  ])('returns [] when %s (契约 §2.2 缺失/非数组 → [])', async (_label, body, raw) => {
    // Arrange
    stubFetch(200, body, raw);

    // Act
    const { value, error } = await capture(() => listAvailableModels());

    // Assert
    expect(error, `不得抛错：${String(error)}`).toBeUndefined();
    expect(value).toEqual([]);
  });

  it('rejects with the frozen error envelope (code + status) on non-2xx — never swallows to []', async () => {
    // Arrange
    stubFetch(500, { error: { code: 'upstream_error', message: 'boom message' } });

    // Act
    const { value, error } = await capture(() => listAvailableModels());

    // Assert
    expect(value, '非 2xx 严禁吞错返回 []').toBeUndefined();
    expect(error).toBeInstanceOf(Error);
    expect((error as Error).message).toBe('boom message');
    expect((error as Error & { code?: unknown }).code).toBe('upstream_error');
    expect((error as Error & { status?: unknown }).status).toBe(500);
  });

  it('surfaces 401 envelope code invalid_api_key as a rejection', async () => {
    // Arrange
    stubFetch(401, { error: { code: 'invalid_api_key', message: 'invalid token' } });

    // Act
    const { value, error } = await capture(() => listAvailableModels());

    // Assert
    expect(value).toBeUndefined();
    expect((error as Error & { code?: unknown })?.code).toBe('invalid_api_key');
    expect((error as Error & { status?: unknown })?.status).toBe(401);
  });
});
