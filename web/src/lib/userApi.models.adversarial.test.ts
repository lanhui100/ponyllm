// @vitest-environment happy-dom
// wave-6 L2-AT 前端对抗测试：listAvailableModels() 的畸形载荷 / 失败注入 / 确定性竞态。
//
// 契约：.dev-team/contracts/wave-6-user-models-picker.md §2.2（返回值恒为 UserModelOption[]）
// + task-2 交付物 4（畸形载荷不得崩溃且 Console Error=0；失败必须以受控 Error 失败而非挂起；
//   固定 20 路并发 + barrier，断言无未处理 rejection、结果确定）。
//
// 断言依据（防歧义）：
// - "models 缺失 / null / 非数组 → []"：契约 §2.2 逐字。
// - 元素级类型不变式（id: string / display_name: string|null / provider: string）由**冻结的
//   TS 接口 `UserModelOption`** 直接推出：返回 `null` 元素或 `display_name: 42` 会违反该接口，
//   因此这里断言类型不变式（不绑定具体归一化取值）。
// - 红相基线（T1 空桩 throw NotImplemented）：本文件必须真实 FAIL（非编译错误）。

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { listAvailableModels, type UserModelOption } from './userApi';

interface FetchCall {
  url: string;
  init?: RequestInit;
}

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

async function capture<T>(fn: () => Promise<T>): Promise<{ value?: T; error?: unknown }> {
  try {
    return { value: await fn() };
  } catch (error) {
    return { error };
  }
}

/** 冻结接口不变式：每个元素必须是可用的 UserModelOption。 */
function assertFrozenOptionShape(rows: unknown[], label: string): void {
  for (const row of rows) {
    expect(row, `${label}: 元素不得为 null/非对象（冻结接口为 UserModelOption[]）`).toBeTypeOf(
      'object',
    );
    expect(row).not.toBeNull();
    const opt = row as Partial<UserModelOption>;
    expect(typeof opt.id, `${label}: id 必须是 string（冻结接口）`).toBe('string');
    expect(
      opt.display_name === null || typeof opt.display_name === 'string',
      `${label}: display_name 必须是 string|null（冻结接口）；got ${JSON.stringify(opt.display_name)}`,
    ).toBe(true);
    expect(typeof opt.provider, `${label}: provider 必须是 string（冻结接口）`).toBe('string');
  }
}

describe('listAvailableModels 对抗：畸形载荷注入', () => {
  let consoleError: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it.each([
    ['models = string', { models: 'nope' }],
    ['models = object', { models: { id: 'x' } }],
    ['models = null', { models: null }],
    ['models missing', {}],
    ['body = null (invalid JSON)', undefined],
  ])('%s → 返回 [] 且不崩溃 / Console Error=0', async (_label, body) => {
    // Arrange
    const raw = body === undefined ? '<<not json>>' : undefined;
    stubFetch(200, body, raw);

    // Act
    const { value, error } = await capture(() => listAvailableModels());

    // Assert
    expect(error, `不得抛错：${String(error)}`).toBeUndefined();
    expect(value).toEqual([]);
    expect(consoleError).not.toHaveBeenCalled();
  });

  it.each([
    ['null element', [null], undefined],
    ['missing id', [{ provider: 'openai' }], undefined],
    ['non-object element', ['gpt-4o'], undefined],
    [
      'valid element among junk',
      [null, 'junk', { id: 'gpt-4o', provider: 'openai', display_name: 'GPT-4o' }],
      [{ id: 'gpt-4o', provider: 'openai', display_name: 'GPT-4o' }],
    ],
    ['wrong-typed fields', [{ id: 'gpt-4o', display_name: 42, provider: 7 }], undefined],
  ])(
    '元素级畸形（%s）→ 不崩溃、类型不变式成立、合法元素原样保留',
    async (_label, models, expectExact) => {
      // Arrange
      stubFetch(200, { models });

      // Act
      const { value, error } = await capture(() => listAvailableModels());

      // Assert
      expect(error, `不得抛错：${String(error)}`).toBeUndefined();
      expect(Array.isArray(value)).toBe(true);
      assertFrozenOptionShape(value as unknown[], String(_label));
      if (expectExact !== undefined) {
        expect(value).toEqual(expectExact);
      }
      expect(consoleError).not.toHaveBeenCalled();
    },
  );
});

describe('listAvailableModels 对抗：超时 / 失败注入（受控失败，绝不挂起）', () => {
  beforeEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it('fetch rejects with AbortError → 必须以 Error 拒绝（若挂起，本用例超时失败）', async () => {
    // Arrange
    let fetchCalls = 0;
    const abortError = new Error('The operation was aborted.');
    abortError.name = 'AbortError';
    vi.stubGlobal('fetch', async () => {
      fetchCalls += 1;
      throw abortError;
    });

    // Act
    const { value, error } = await capture(() => listAvailableModels());

    // Assert
    expect(fetchCalls, '必须发出真实 fetch（红相：空桩 NotImplemented → 0 次 → FAIL）').toBe(1);
    expect(value, '契约 §2.2：严禁吞错返回 []').toBeUndefined();
    expect(error, '必须以受控 Error 失败（挂起则本用例超时）').toBeInstanceOf(Error);
  });

  it('fetch rejects with a network TypeError → 必须以 Error 拒绝', async () => {
    // Arrange
    let fetchCalls = 0;
    vi.stubGlobal('fetch', async () => {
      fetchCalls += 1;
      throw new TypeError('Failed to fetch');
    });

    // Act
    const { value, error } = await capture(() => listAvailableModels());

    // Assert
    expect(fetchCalls, '必须发出真实 fetch（红相：空桩 → 0 次 → FAIL）').toBe(1);
    expect(value).toBeUndefined();
    expect(error).toBeInstanceOf(Error);
  });

  it('500 错误信封 → 受控 Error 且 code/status 已填（不得返回 []）', async () => {
    // Arrange
    stubFetch(500, { error: { code: 'gateway_error', message: 'upstream down' } });

    // Act
    const { value, error } = await capture(() => listAvailableModels());

    // Assert
    expect(value).toBeUndefined();
    expect(error).toBeInstanceOf(Error);
    expect((error as Error & { code?: unknown }).code).toBe('gateway_error');
    expect((error as Error & { status?: unknown }).status).toBe(500);
  });
});

describe('listAvailableModels 对抗：确定性竞态（20 路并发 + barrier）', () => {
  const CONCURRENCY = 20;

  beforeEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it('20 路并发同时起跑：全部成功、结果逐字节一致、零未处理 rejection', async () => {
    // Arrange
    const unhandled: unknown[] = [];
    const onUnhandled = (event: PromiseRejectionEvent) => {
      unhandled.push(event.reason);
    };
    window.addEventListener('unhandledrejection', onUnhandled);

    const payload = {
      models: [
        { id: 'auto', provider: 'ponyllm', display_name: 'Auto' },
        { id: 'gpt-4o', provider: 'openai', display_name: null },
      ],
    };
    const calls = stubFetch(200, payload);

    let release!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });

    // Act：20 个调用全部挂在同一 barrier 上，release 后同时起跑（确定性，无 sleep）
    const pending = Array.from({ length: CONCURRENCY }, () =>
      gate.then(() => listAvailableModels()),
    );
    release();
    const settled = await capture(() => Promise.all(pending));

    // Assert
    window.removeEventListener('unhandledrejection', onUnhandled);
    expect(settled.error, `并发调用不得有未处理 rejection：${String(settled.error)}`).toBeUndefined();
    expect(unhandled, `零未处理 rejection；got ${JSON.stringify(unhandled)}`).toEqual([]);
    expect(calls.length, '每次调用都必须发出独立 fetch').toBe(CONCURRENCY);

    const results = settled.value as UserModelOption[][];
    expect(results).toHaveLength(CONCURRENCY);
    const serialized = new Set(results.map((r) => JSON.stringify(r)));
    expect(serialized.size, '并发结果必须完全确定（逐字节一致）').toBe(1);
    expect(results[0]).toEqual([
      { id: 'auto', provider: 'ponyllm', display_name: 'Auto' },
      { id: 'gpt-4o', provider: 'openai', display_name: null },
    ]);
    assertFrozenOptionShape(results[0] as unknown[], 'concurrency');
  });
});
