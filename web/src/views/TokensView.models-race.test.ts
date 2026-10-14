// @vitest-environment happy-dom
// wave-6 缺陷回归红相：TokensView `loadModels()` 单飞（single-flight）+ 请求代次守卫。
//
// 触发源（双路 FAIL，Lead 亲验）：
//   - L3-A `.dev-team/review-wave6-a.json`：陈旧响应无条件回写共享 ref（覆盖较新成功）。
//   - L3-P 五问第 4 问 `concurrency_lock.frontend_concurrency`：开合弹窗 3 次 →
//     `listAvailableModels` 调用 4 次（期望 1），缺单飞。
//
// 冻结契约 v2（Lead 裁决，以此为准）：
//   C1 单飞：在途请求存在时 `loadModels()` 复用同一在途 Promise，不得发起第二次调用。
//   C2 代次守卫：每次真正发起的请求分配单调递增 seq；仅最新一代可回写
//      `models`/`modelsError`/`modelsLoading`；陈旧响应（成功或失败）静默丢弃。
//   C3 页面级缓存：模型清单不因下拉面板关闭而取消/丢弃（onMounted 预载时面板本就是关闭的）。
//   C4 失败降级不变：失败 → 提示 + 可提交；点击重试 = 发起新一代请求。
//
// 本文件用**可控 deferred promise** 精确控时序（零 sleep、零随机），断言 R1~R4。
// 红相要求：当前实现（无单飞、无代次）下必须**真实 FAIL**（断言失败，非编译错误）。

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { createApp, nextTick } from 'vue';
import { createPinia, setActivePinia } from 'pinia';
import TokensView from './TokensView.vue';
import * as userApi from '../lib/userApi';

vi.mock('../components/NavBar.vue', () => ({
  default: {
    name: 'NavBar',
    template: '<nav data-testid="mock-navbar"></nav>',
  },
}));

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason: unknown) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const MODEL_A: userApi.UserModelOption = { id: 'a', provider: 'ponyllm', display_name: 'One' };
const MODEL_B: userApi.UserModelOption = { id: 'b', provider: 'openai', display_name: 'Two' };

const FAILURE_TEXT = '模型列表加载失败';

describe('TokensView loadModels 单飞 + 代次守卫（wave-6 缺陷回归红相）', () => {
  let pinia: ReturnType<typeof createPinia>;
  let container: HTMLDivElement;
  let app: ReturnType<typeof createApp>;
  /** 每次**真实发起**的 `listAvailableModels()` 调用对应的 deferred（按调用顺序）。 */
  let pending: Array<Deferred<userApi.UserModelOption[]>>;

  beforeEach(() => {
    vi.clearAllMocks();
    pinia = createPinia();
    setActivePinia(pinia);
    container = document.createElement('div');
    document.body.appendChild(container);
    pending = [];

    vi.spyOn(userApi, 'listMyTokens').mockResolvedValue([]);
    vi.spyOn(userApi, 'createMyToken').mockResolvedValue({
      key_id: 'tk-new',
      api_key: 'sk-pony-race-secret-1234',
      name: 'race-token',
      model_limits: null,
      quota: null,
      expires_at: null,
      enabled: true,
      used_tokens: 0,
    });
    // 关键：模型清单请求完全由测试控时序（不自动 settle），并据此统计调用次数。
    vi.spyOn(userApi, 'listAvailableModels').mockImplementation(() => {
      const d = deferred<userApi.UserModelOption[]>();
      pending.push(d);
      return d.promise;
    });
  });

  afterEach(() => {
    if (app) {
      app.unmount();
    }
    if (container && container.parentNode) {
      document.body.removeChild(container);
    }
    vi.restoreAllMocks();
  });

  /** 确定性 flush：只推进微任务与 Vue 调度，不使用 sleep。 */
  async function flush(): Promise<void> {
    await Promise.resolve();
    await Promise.resolve();
    await nextTick();
    await nextTick();
  }

  async function mountView(): Promise<void> {
    app = createApp(TokensView);
    app.use(pinia);
    app.mount(container);
    await flush();
  }

  /** `listAvailableModels` 的真实调用次数（单飞契约的核心计量）。 */
  function callCount(): number {
    return vi.mocked(userApi.listAvailableModels).mock.calls.length;
  }

  function openCreateModal(): void {
    const openBtn = container.querySelector(
      '[data-testid="open-create-token-btn"]',
    ) as HTMLButtonElement;
    expect(openBtn, '新增 Token 按钮必须存在').not.toBeNull();
    openBtn.click();
  }

  function closeCreateModal(): void {
    const cancel = Array.from(container.querySelectorAll('button')).find((b) =>
      b.textContent?.includes('取消'),
    );
    expect(cancel, '取消按钮必须存在').toBeDefined();
    cancel!.click();
  }

  function openModelPicker(): void {
    const trigger = container.querySelector(
      '[data-testid="token-models-select"]',
    ) as HTMLButtonElement | null;
    expect(trigger, '模型下拉触发器必须存在').not.toBeNull();
    trigger!.click();
  }

  function optionEls(): HTMLElement[] {
    return Array.from(
      container.querySelectorAll('[data-testid="token-models-option"]'),
    ) as HTMLElement[];
  }

  function submitBtn(): HTMLButtonElement {
    return container.querySelector(
      '[data-testid="submit-create-token-btn"]',
    ) as HTMLButtonElement;
  }

  // -------------------------------------------------------------------------
  // R1（C1，核心红相）：并发触发必须单飞 → 调用次数 == 1
  // -------------------------------------------------------------------------
  it('R1 并发触发单飞：mount + 打开弹窗只发起 1 次请求，结果照常渲染', async () => {
    // Arrange：mount 触发请求 #1（onMounted → loadModels），保持 pending
    await mountView();
    expect(pending.length, 'mount 必须发起请求 #1').toBe(1);

    // Act：请求 #1 在途时打开新增弹窗（再次调用 loadModels）
    openCreateModal();
    await flush();

    // Assert 1（核心红相）：C1 单飞 —— 在途请求存在时不得发起第二次调用
    expect(
      callCount(),
      `C1 单飞：在途请求存在时 loadModels 必须复用同一在途 Promise，` +
        `不得再次调用 listAvailableModels（当前实现为 2 次 → 红）`,
    ).toBe(1);
    expect(pending.length).toBe(1);

    // Assert 2：在途请求 resolve 后，弹窗下拉必须照常渲染该结果
    pending[0].resolve([MODEL_A, MODEL_B]);
    await flush();
    openModelPicker();
    await flush();
    expect(optionEls().length, '单飞复用的在途结果必须被渲染').toBe(2);
    expect(container.textContent).not.toContain(FAILURE_TEXT);

    // Assert 3：选择/chips 仍可用
    optionEls()[0].click();
    await flush();
    expect(container.querySelectorAll('[data-testid="token-models-chip"]').length).toBe(1);
  });

  // -------------------------------------------------------------------------
  // R2（C2）：最新一代语义 —— 成功可回写；后续新一代失败则清空并提示
  // -------------------------------------------------------------------------
  it('R2 代次守卫：最新一代成功可回写，后续新一代失败才清空并提示', async () => {
    // Arrange：mount → #1 pending；打开弹窗（C1：仍复用 #1）
    await mountView();
    openCreateModal();
    await flush();
    expect(callCount(), 'C1：打开弹窗不得叠加第二路请求').toBe(1);

    // Act 1：最新（也是唯一在途）#1 成功
    pending[0].resolve([MODEL_A, MODEL_B]);
    await flush();

    // Assert 1：成功结果回写，且不得出现失败提示
    openModelPicker();
    await flush();
    expect(optionEls().length).toBe(2);
    expect(
      container.textContent,
      '成功一代不得渲染「模型列表加载失败」',
    ).not.toContain(FAILURE_TEXT);

    // Act 2：关闭再重开弹窗 → 此时无在途请求 → 发起新一代 #2，并让其失败
    closeCreateModal();
    await flush();
    openCreateModal();
    await flush();
    expect(callCount(), '重开弹窗（无在途请求）必须发起新一代请求').toBe(2);
    pending[1].reject(new Error('newest generation failed'));
    await flush();

    // Assert 2：最新一代失败 → 错误提示出现 + models 清空
    expect(container.textContent, '最新一代失败必须提示').toContain(FAILURE_TEXT);
    openModelPicker();
    await flush();
    expect(optionEls().length, '最新一代失败必须清空 models').toBe(0);
    expect(container.textContent).toContain('没有匹配的模型');
  });

  // -------------------------------------------------------------------------
  // R3（C3）：面板关闭不得取消/丢弃在途响应（页面级缓存）
  // -------------------------------------------------------------------------
  it('R3 页面级缓存：关闭弹窗不丢弃在途响应，响应仍按最新一代回写', async () => {
    // Arrange：mount → #1 pending（面板本就是关闭态）
    await mountView();
    expect(callCount()).toBe(1);

    // Act：打开弹窗（C1 复用 #1）→ 在 #1 仍 pending 时关闭弹窗 → 再 resolve #1
    openCreateModal();
    await flush();
    expect(callCount(), 'C1：打开弹窗不得叠加第二路请求').toBe(1);
    closeCreateModal();
    await flush();
    expect(
      container.querySelector('[data-testid="create-token-modal"]'),
      '弹窗必须已关闭',
    ).toBeNull();
    pending[0].resolve([MODEL_A, MODEL_B]);
    await flush();

    // Assert：重新打开弹窗 → 在途响应未被丢弃，结果已回写
    openCreateModal();
    await flush();
    expect(callCount(), '重开弹窗（无在途请求）应发起新一代 #2').toBe(2);
    openModelPicker();
    await flush();
    expect(
      optionEls().length,
      'C3：面板关闭不得丢弃在途响应，结果必须已回写（页面级缓存）',
    ).toBe(2);
    expect(container.textContent).not.toContain(FAILURE_TEXT);
  });

  // -------------------------------------------------------------------------
  // R4（C4）：失败降级不变 + 重试 = 新一代
  // -------------------------------------------------------------------------
  it('R4 失败降级：提示可见、提交可用；点击重试发起新一代并正确渲染', async () => {
    // Arrange：mount → #1 pending；打开弹窗（C1 复用 #1）
    await mountView();
    openCreateModal();
    await flush();
    expect(callCount(), 'C1：打开弹窗不得叠加第二路请求').toBe(1);

    // Act 1：让最新一代失败
    pending[0].reject(new Error('models unavailable'));
    await flush();

    // Assert 1：降级提示 + 重试入口 + 提交按钮未禁用（契约 §2.5 / C4）
    expect(container.textContent).toContain(FAILURE_TEXT);
    const retry = container.querySelector(
      '[data-testid="token-models-retry"]',
    ) as HTMLButtonElement | null;
    expect(retry, 'C4：失败必须提供重试入口').not.toBeNull();
    expect(submitBtn().disabled, 'C4：模型列表失败不得禁用提交按钮').toBe(false);

    // Act 2：点击重试 → 发起新一代 #2 并成功
    retry!.click();
    await flush();
    expect(callCount(), 'C4：点击重试必须发起新一代请求（调用数 +1）').toBe(2);
    pending[1].resolve([MODEL_A, MODEL_B]);
    await flush();

    // Assert 2：恢复为成功态
    expect(container.textContent).not.toContain(FAILURE_TEXT);
    expect(
      container.querySelector('[data-testid="token-models-retry"]'),
      '成功后重试入口必须消失',
    ).toBeNull();
    openModelPicker();
    await flush();
    expect(optionEls().length).toBe(2);
    expect(submitBtn().disabled).toBe(false);
  });
});
