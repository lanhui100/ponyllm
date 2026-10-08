// @vitest-environment happy-dom
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { createApp, nextTick } from 'vue';
import AntigravityPoolCard from './AntigravityPoolCard.vue';
import { markQuotaProbed, clearQuotaProbedAt } from '../composables/useAdminConfig';
import type { KeyView, KeyTestView } from '../types/admin';

describe('AntigravityPoolCard Component', () => {
  beforeEach(() => {
    clearQuotaProbedAt();
  });

  it('renders upstream eligibility freeze as a red error cell, not mint-green cooling', async () => {
    const container = document.createElement('div');
    document.body.appendChild(container);

    const keys: KeyView[] = [
      {
        id: 'ag-noteligible@gmail.com',
        provider: 'antigravity',
        masked_key: 'ya29.***',
        state: 'cooling_down',
        priority: 1,
        weight: 10,
        cooldown_remaining_secs: 3 * 24 * 3600, // ~3 days
        cooldown_reason: 'eligibility',
        error_message: 'Your current account is not eligible for Gemini Code Assist for individuals.',
      },
      {
        id: 'ag-good@gmail.com',
        provider: 'antigravity',
        masked_key: 'ya29.***',
        state: 'cooling_down',
        priority: 2,
        weight: 10,
        cooldown_remaining_secs: 60,
        // No cooldown_reason: soft quota/rate-limit cooling stays mint-green.
      },
    ];

    const app = createApp(AntigravityPoolCard, {
      keys,
      keyTestResults: {},
      adminWriteEnabled: true,
    });
    app.mount(container);
    await nextTick();

    const cells = container.querySelectorAll('[data-testid="slot-heatmap-cell"]');
    expect(cells.length).toBe(2);

    // Eligibility-frozen account: red error block.
    expect(cells[0].className).toContain('bg-rose-600');

    // Soft cooling account: mint-green cooling block (unchanged).
    expect(cells[1].className).toContain('bg-[#a3e4a8]');

    // Hover the eligibility cell: tooltip names the freeze and the upstream reason.
    cells[0].dispatchEvent(new MouseEvent('mouseenter', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 220));
    await nextTick();
    const tooltip = document.body.querySelector('[data-testid="ui-tooltip"]');
    expect(tooltip).not.toBeNull();
    const tooltipText = tooltip?.textContent ?? '';
    expect(tooltipText).toContain('资格受限');
    expect(tooltipText).toContain('not eligible');
    expect(tooltipText).toContain('后解冻');

    app.unmount();
    document.body.removeChild(container);
  });

  it('keeps the full freeze tooltip for a dial-tested frozen account, and never shows stale red on an active key', async () => {
    const container = document.createElement('div');
    document.body.appendChild(container);

    const keys: KeyView[] = [
      {
        id: 'ag-dial-tested@gmail.com',
        provider: 'antigravity',
        masked_key: 'ya29.***',
        state: 'cooling_down',
        priority: 1,
        weight: 10,
        cooldown_remaining_secs: 3 * 24 * 3600,
        cooldown_reason: 'eligibility',
        error_message: 'Your current account is not eligible for Gemini Code Assist for individuals.',
      },
      {
        id: 'ag-stale-probe@gmail.com',
        provider: 'antigravity',
        masked_key: 'ya29.***',
        state: 'active',
        priority: 2,
        weight: 10,
      },
    ];

    const keyTestResults: Record<string, KeyTestView> = {
      // 后端拨测已冻结并在 keyTestResults 持久化资格失败；state 也已是
      // 冷却+资格 → 必须走"完整冻结文案"（含解冻倒计时），不是探测分支
      // 的无倒计时短文案（review P1 回归）。
      'ag-dial-tested@gmail.com': {
        success: false,
        latency_ms: 200,
        message: 'quota fetch error: 403 Your current account is not eligible...',
        error_code: 'eligibility_frozen',
      },
      // 仅陈旧探针证据而 state 已回 active：不得硬判红（冻结可能已解除）。
      'ag-stale-probe@gmail.com': {
        success: false,
        latency_ms: 200,
        message: 'quota fetch error: 403 not eligible for Gemini Code Assist',
        error_code: 'eligibility_frozen',
      },
    };

    const app = createApp(AntigravityPoolCard, {
      keys,
      keyTestResults,
      adminWriteEnabled: true,
    });
    app.mount(container);
    await nextTick();

    const cells = container.querySelectorAll('[data-testid="slot-heatmap-cell"]');
    expect(cells.length).toBe(2);

    // Dial-tested frozen account: red + aria/文案含"已冻结跳过"与倒计时。
    expect(cells[0].className).toContain('bg-rose-600');
    expect(cells[0].getAttribute('aria-label')).toContain('资格受限');
    cells[0].dispatchEvent(new MouseEvent('mouseenter', { bubbles: true }));
    await new Promise((r) => setTimeout(r, 220));
    await nextTick();
    const tooltip = document.body.querySelector('[data-testid="ui-tooltip"]');
    const tooltipText = tooltip?.textContent ?? '';
    expect(tooltipText).toContain('已冻结跳过');
    expect(tooltipText).toContain('后解冻');

    // Stale active key with a persisted eligibility probe failure: NOT red.
    // The backend says this key is active again, so the 6h-old probe result
    // must not paint a red "拒绝服务" block over a schedulable account.
    expect(cells[1].className).not.toContain('bg-rose-600');

    app.unmount();
    document.body.removeChild(container);
  });
  it('renders correctly with mixed active and cooling keys', async () => {
    const container = document.createElement('div');
    document.body.appendChild(container);

    const keys: KeyView[] = [
      {
        id: 'acc-1',
        provider: 'antigravity',
        masked_key: 'ya29.***',
        state: 'active',
        priority: 1,
        weight: 10,
      },
      {
        id: 'acc-2',
        provider: 'antigravity',
        masked_key: 'ya29.***',
        state: 'cooling_down',
        priority: 1,
        weight: 10,
        cooldown_remaining_secs: 120, // 2 minutes
      },
    ];

    const keyTestResults: Record<string, KeyTestView> = {
      'acc-1': {
        success: true,
        latency_ms: 100,
        message: 'ok',
        quota_groups: [
          {
            display_name: 'Gemini 2.5/3.0',
            buckets: [
              {
                bucket_id: 'gemini-5h',
                window: '5h',
                remaining_fraction: 0.8,
                time_until_reset: '3小时后重置',
              },
              {
                bucket_id: 'gemini-weekly',
                window: 'weekly',
                remaining_fraction: 0.9,
                time_until_reset: '4天后重置',
              },
            ],
          },
          {
            display_name: 'Claude 3.7 / 3.5 Sonnet',
            buckets: [
              {
                bucket_id: 'claude-5h',
                window: '5h',
                remaining_fraction: 0.6,
                time_until_reset: '1小时后重置',
              },
              {
                bucket_id: 'claude-weekly',
                window: 'weekly',
                remaining_fraction: 0.75,
                time_until_reset: '2天后重置',
              },
            ],
          },
        ],
      },
    };

    const refreshSpy = vi.fn();
    const navigateSpy = vi.fn();

    markQuotaProbed('acc-1');
    const app = createApp(AntigravityPoolCard, {
      keys,
      keyTestResults,
      adminWriteEnabled: true,
      onRefreshQuotas: refreshSpy,
      onNavigateGovernance: navigateSpy,
    });

    app.mount(container);
    await nextTick();

    // 1. Verify Card Title & Header status
    expect(container.textContent).toContain('Antigravity 算力池');
    expect(container.textContent).toContain('账户可用性状态');
    expect(container.textContent).toContain('1/2 账号就绪');

    // 2. Verify Availability Rate (1 active / 2 total = 50%)
    expect(container.textContent).toContain('50%');
    // Ensure "状态良好" status label is removed
    expect(container.textContent).not.toContain('状态良好');

    // Verify GitHub-style heatmap cells
    const cells = container.querySelectorAll('[data-testid="slot-heatmap-cell"]');
    expect(cells.length).toBe(2);
    // Verify empty placeholder slots are rendered (2 groups of 80 = 160 total - 2 active = 158 empty)
    const emptySlots = container.querySelectorAll('[data-testid="slot-heatmap-empty"]');
    expect(emptySlots.length).toBe(158);

    const grids = container.querySelectorAll('[data-testid="slot-heatmap-grid"]');
    expect(grids.length).toBe(2);
    expect(grids[0]?.className).toContain('grid-rows-5');
    expect(grids[1]?.className).toContain('grid-rows-5');

    // 3. Verify Next recovery countdown hint
    expect(container.textContent).toContain('最近解冻');
    expect(container.textContent).toContain('2分');
    expect(container.textContent).toContain('acc-2');

    // 4. Verify 5h rolling capacities
    expect(container.textContent).toContain('5小时窗口');
    expect(container.textContent).toContain('80%'); // Gemini 5h
    expect(container.textContent).toContain('3小时后重置');
    // 上游仍携带 Claude 分组（回归素材），但 UI 已不再查询显示 Claude 额度
    expect(container.textContent).not.toContain('Claude');

    // 5. Verify Weekly rolling capacities
    expect(container.textContent).toContain('周度窗口');
    expect(container.textContent).toContain('90%'); // Gemini Weekly

    // 6. Test Refresh button click
    const refreshBtn = container.querySelector('[data-testid="refresh-antigravity-pool-btn"]') as HTMLButtonElement;
    expect(refreshBtn).not.toBeNull();
    refreshBtn.click();
    expect(refreshSpy).toHaveBeenCalledTimes(1);

    app.unmount();
    document.body.removeChild(container);
  });

  it('handles all cooling accounts gracefully', async () => {
    const container = document.createElement('div');
    document.body.appendChild(container);

    const keys: KeyView[] = [
      {
        id: 'acc-1',
        provider: 'antigravity',
        masked_key: 'ya29.***',
        state: 'cooling_down',
        priority: 1,
        weight: 10,
        cooldown_remaining_secs: 45,
      },
    ];

    const app = createApp(AntigravityPoolCard, {
      keys,
      keyTestResults: {},
      adminWriteEnabled: true,
    });

    app.mount(container);
    await nextTick();

    expect(container.textContent).toContain('0/1 账号就绪');
    expect(container.textContent).toContain('0%');
    expect(container.textContent).not.toContain('全部限流');
    expect(container.textContent).toContain('最近解冻:');
    expect(container.textContent).toContain('秒');
    expect(container.textContent).toContain('所有账号冷却中');

    app.unmount();
    document.body.removeChild(container);
  });

  it('treats active key with flat quota only (no quota_groups) as ready, not cooling', async () => {
    const container = document.createElement('div');
    document.body.appendChild(container);

    // 实证回归：retrieveUserQuotaSummary 4s 超时缺席时，后端只回 fetchAvailableModels
    // 平铺 33 模型（无周窗口标识）。此前 weeklyFraction 恒为初始 0，被误判冷却、
    // 踢出聚合导致水位锁死；修复后未知周默认健康，冷却只跟随真实 state。
    const keys: KeyView[] = [
      {
        id: 'ag-active-flat@gmail.com',
        provider: 'antigravity',
        masked_key: '1//***',
        state: 'active',
        priority: 1,
        weight: 10,
      },
    ];

    const keyTestResults: Record<string, KeyTestView> = {
      'ag-active-flat@gmail.com': {
        success: true,
        latency_ms: 5000,
        message: 'probe ok (quota fetched for 33 models)',
        quota: [
          { model_id: 'chat_20706', remaining_fraction: 1.0 },
          { model_id: 'claude-sonnet-4-6', remaining_fraction: 1.0, time_until_reset: '4小时59分后' },
          // 最小值证伪：末项故意放 50%，若实现退化为遍历覆盖/末值胜出则得 50% 而非 1%
          { model_id: 'gemini-2.5-flash', remaining_fraction: 0.0068, time_until_reset: '1小时38分后' },
          { model_id: 'gemini-3.8-flash-high', remaining_fraction: 0.5, time_until_reset: '1小时38分后' },
        ],
      },
    };

    const app = createApp(AntigravityPoolCard, {
      keys,
      keyTestResults,
      adminWriteEnabled: true,
    });
    markQuotaProbed('ag-active-flat@gmail.com');

    app.mount(container);
    await nextTick();

    // 未知周不再误判：active 账号必须计入就绪，而非“所有账号冷却中”
    expect(container.textContent).toContain('1/1 账号就绪');
    expect(container.textContent).not.toContain('所有账号冷却中');
    // 5h 取同系列最小值（与模型管理页取最小值语义一致）：0.0068*100=0.68→1%
    // 精确断言 testid，避免与可用率/周水位的混淆；周缺席显示"--"（未知不再伪装 100%）
    expect(container.querySelector('[data-testid="gemini-h5-percent"]')?.textContent).toContain('1%');
    expect(container.querySelector('[data-testid="gemini-weekly-percent"]')?.textContent).toContain('--');
    // Claude 系列不再展示：claude 模型 id 被跳过，claude testid 已移除
    expect(container.querySelector('[data-testid="claude-h5-percent"]')).toBeNull();
    expect(container.querySelector('[data-testid="claude-weekly-percent"]')).toBeNull();

    app.unmount();
    document.body.removeChild(container);
  });

  it('renders account details modal when a slot is clicked', async () => {
    const container = document.createElement('div');
    document.body.appendChild(container);

    const keys: KeyView[] = [
      {
        id: 'ag-pro-account@gmail.com',
        provider: 'antigravity',
        masked_key: 'ya29.***',
        state: 'active',
        priority: 1,
        weight: 10,
        usage: {
          window_5h: {
            prompt_tokens: 80000,
            completion_tokens: 20000,
            cached_tokens: 5000,
            total_tokens: 100000,
            requests: 25,
          },
          window_weekly: {
            prompt_tokens: 400000,
            completion_tokens: 100000,
            cached_tokens: 20000,
            total_tokens: 500000,
            requests: 120,
          },
          window_monthly: {
            prompt_tokens: 1600000,
            completion_tokens: 400000,
            cached_tokens: 80000,
            total_tokens: 2000000,
            requests: 480,
          },
          estimated_capacity_5h: 500000,
          estimated_tokens_remaining_5h: 400000,
          account_tier: 'pro',
          confidence: 0.95,
        },
      },
    ];

    const app = createApp(AntigravityPoolCard, {
      keys,
      keyTestResults: {},
      adminWriteEnabled: true,
    });

    app.mount(container);
    await nextTick();

    // Verify cell exists
    const cell = container.querySelector('[data-testid="slot-heatmap-cell"]') as HTMLElement;
    expect(cell).not.toBeNull();

    // Verify factual cycle summary rendered in simplified cards
    expect(container.textContent).toContain('5小时');
    expect(container.textContent).toContain('80K');
    expect(container.textContent).toContain('20K');
    expect(container.textContent).toContain('5K');
    expect(container.textContent).toContain('25次');

    // Click heatmap cell to open single account profile drawer
    cell.click();
    await nextTick();

    expect(container.querySelector('[data-testid="single-account-detail-card"]')).not.toBeNull();
    expect(container.textContent).toContain('单账号周期额度画像');
    expect(container.textContent).toContain('输入 (Prompt)');
    expect(container.textContent).toContain('输出 (Completion)');
    expect(container.textContent).toContain('缓存命中 (Cached)');
    expect(container.textContent).toContain('累计调用次数 (Requests)');

    // Click again to toggle close
    cell.click();
    await nextTick();
    expect(container.querySelector('[data-testid="single-account-detail-card"]')).toBeNull();

    app.unmount();
    document.body.removeChild(container);
  });

  it('handles account with completely missing usage gracefully without throwing', async () => {
    const container = document.createElement('div');
    document.body.appendChild(container);

    const keys: KeyView[] = [
      {
        id: 'acc-bare-key',
        provider: 'antigravity',
        state: 'active',
        priority: 1,
        weight: 10,
        masked_key: 'bare-key-secret',
        usage: null,
      },
    ];

    const app = createApp(AntigravityPoolCard, {
      keys,
      keyTestResults: {},
      adminWriteEnabled: true,
    });

    app.mount(container);
    await nextTick();

    const cell = container.querySelector('[data-testid="slot-heatmap-cell"]') as HTMLElement;
    expect(cell).not.toBeNull();

    // Clicking bare account should safely render drawer without runtime exceptions
    cell.click();
    await nextTick();

    expect(container.querySelector('[data-testid="single-account-detail-card"]')).not.toBeNull();
    expect(container.textContent).toContain('单账号周期额度画像 · acc-bare-key');
    expect(container.textContent).toContain('动态校准中 (CALIBRATING)');

    app.unmount();
    document.body.removeChild(container);
  });

  it('renders differentiated heat classes and tooltips for validation required, invalid grant, and failed probe', async () => {
    const container = document.createElement('div');
    document.body.appendChild(container);

    const keys: KeyView[] = [
      {
        id: 'ag-val@gmail.com',
        provider: 'antigravity',
        state: 'disabled',
        priority: 1,
        weight: 10,
        masked_key: 'ya29.***',
        disabled_reason: 'Google account verification required (VALIDATION_REQUIRED): Verify your account to continue.',
      },
      {
        id: 'ag-burned@gmail.com',
        provider: 'antigravity',
        state: 'disabled',
        priority: 2,
        weight: 10,
        masked_key: 'ya29.***',
        disabled_reason: 'OAuth refresh rejected (400 Bad Request): {"error": "invalid_grant"}',
      },
      {
        id: 'ag-probe-val@gmail.com',
        provider: 'antigravity',
        state: 'active',
        priority: 3,
        weight: 10,
        masked_key: 'ya29.***',
      },
    ];

    const keyTestResults: Record<string, KeyTestView> = {
      'ag-probe-val@gmail.com': {
        success: false,
        latency_ms: 150,
        message: 'quota fetch error: 403 VALIDATION_REQUIRED',
        error_code: 'account_validation_required',
      },
    };

    markQuotaProbed('ag-probe-val@gmail.com');
    const app = createApp(AntigravityPoolCard, {
      keys,
      keyTestResults,
      adminWriteEnabled: true,
    });

    app.mount(container);
    await nextTick();

    const cells = container.querySelectorAll('[data-testid="slot-heatmap-cell"]');
    expect(cells.length).toBe(3);

    // Cell 0: disabled with VALIDATION_REQUIRED -> amber
    expect(cells[0].className).toContain('bg-amber-500');

    // Cell 1: disabled with invalid_grant -> rose-500
    expect(cells[1].className).toContain('bg-rose-500');

    // Cell 2: active in backend but probe failed with account_validation_required -> amber
    expect(cells[2].className).toContain('bg-amber-500');

    // Ready accounts must be 0/3, none should be counted as ready
    expect(container.textContent).toContain('0/3 账号就绪');

    app.unmount();
    document.body.removeChild(container);
  });

  it('renders expired cached quota results as pale-green waiting-for-refresh slot (account exists, quota unknown)', async () => {
    const container = document.createElement('div');
    document.body.appendChild(container);

    const keys: KeyView[] = [
      {
        id: 'ag-stale-key@gmail.com',
        provider: 'antigravity',
        state: 'active',
        priority: 1,
        weight: 10,
        masked_key: 'ya29.***',
      },
    ];

    const keyTestResults: Record<string, KeyTestView> = {
      'ag-stale-key@gmail.com': {
        success: true,
        latency_ms: 100,
        message: 'old probe result',
        quota_groups: [
          {
            display_name: 'Gemini',
            buckets: [{ bucket_id: 'b1', window: '5h', remaining_fraction: 0.95 }],
          },
        ],
      },
    };

    // Deliberately do NOT call markQuotaProbed -> cache is treated as stale/unprobed
    const app = createApp(AntigravityPoolCard, {
      keys,
      keyTestResults,
      adminWriteEnabled: true,
    });
    app.mount(container);
    await nextTick();

    // 账号存在但未拿到新鲜额度 -> 极浅绿占位（与"无账号"空槽位的灰阶 #d0d7de 语义区分）
    const cell = container.querySelector('[data-testid="slot-heatmap-cell"]');
    expect(cell?.className).toContain('bg-[#e6f4ea]');
    expect(cell?.className).not.toContain('bg-[#d0d7de]');

    // 无账号的预留空槽位仍为灰阶
    const emptySlots = container.querySelectorAll('[data-testid="slot-heatmap-empty"]');
    expect(emptySlots.length).toBeGreaterThan(0);
    for (const empty of emptySlots) {
      expect((empty as HTMLElement).className).toContain('bg-[#d0d7de]');
      expect((empty as HTMLElement).className).not.toContain('bg-[#e6f4ea]');
    }

    // Water level shows waiting placeholder "--" instead of stale 95%
    expect(container.querySelector('[data-testid="gemini-h5-percent"]')?.textContent).toContain('--');

    app.unmount();
    document.body.removeChild(container);
  });

  it('renders sky-blue heat class for lock_busy replica sync without counting as failed/cooling', async () => {
    const container = document.createElement('div');
    document.body.appendChild(container);

    const keys: KeyView[] = [
      {
        id: 'ag-lock-busy@gmail.com',
        provider: 'antigravity',
        state: 'active',
        priority: 1,
        weight: 10,
        masked_key: 'ya29.***',
      },
    ];

    const keyTestResults: Record<string, KeyTestView> = {
      'ag-lock-busy@gmail.com': {
        success: false,
        latency_ms: 50,
        message: 'serialization lock held by another replica',
        error_code: 'lock_busy',
      },
    };

    markQuotaProbed('ag-lock-busy@gmail.com');
    const app = createApp(AntigravityPoolCard, {
      keys,
      keyTestResults,
      adminWriteEnabled: true,
    });
    app.mount(container);
    await nextTick();

    const cell = container.querySelector('[data-testid="slot-heatmap-cell"]');
    expect(cell?.className).toContain('bg-sky-400');

    // Tolerated as active (not disabled/cooling)
    expect(container.textContent).toContain('1/1 账号就绪');

    app.unmount();
    document.body.removeChild(container);
  });

  it('renders persisted cross-account multi-cycle benchmark as the headline when provided', async () => {
    const container = document.createElement('div');
    document.body.appendChild(container);

    const keys: KeyView[] = [
      {
        id: 'acc-1',
        provider: 'antigravity',
        masked_key: 'ya29.***',
        state: 'active',
        priority: 1,
        weight: 10,
      },
    ];

    // 无实时完成周期、无容量推断：仅靠持久化基准支撑第三栏主数字。
    const keyTestResults: Record<string, KeyTestView> = {};

    const app = createApp(AntigravityPoolCard, {
      keys,
      keyTestResults,
      adminWriteEnabled: true,
      benchmark: {
        persisted_at_ms: 1_700_000_000_000,
        kind_5h: {
          observations: 12,
          avg_tokens: 1_250_000,
          prompt_tokens: 9_000_000,
          completion_tokens: 6_000_000,
          cached_tokens: 0,
          total_tokens: 15_000_000,
          requests: 48,
          completed_cycles: 3,
          avg_completed_tokens: 1_400_000,
          completed_prompt_tokens: 3_600_000,
          completed_completion_tokens: 600_000,
          completed_cached_tokens: 0,
          completed_total_tokens: 4_200_000,
          completed_requests: 9,
          first_observation_ms: 1_600_000_000_000,
          last_observation_ms: 1_700_000_000_000,
        },
        kind_weekly: {
          observations: 2,
          avg_tokens: 7_000_000,
          prompt_tokens: 10_000_000,
          completion_tokens: 4_000_000,
          cached_tokens: 0,
          total_tokens: 14_000_000,
          requests: 20,
          completed_cycles: 0,
          avg_completed_tokens: 0,
          completed_prompt_tokens: 0,
          completed_completion_tokens: 0,
          completed_cached_tokens: 0,
          completed_total_tokens: 0,
          completed_requests: 0,
          first_observation_ms: 1_600_000_000_000,
          last_observation_ms: 1_700_000_000_000,
        },
        kind_monthly: {
          observations: 0,
          avg_tokens: 0,
          prompt_tokens: 0,
          completion_tokens: 0,
          cached_tokens: 0,
          total_tokens: 0,
          requests: 0,
          completed_cycles: 0,
          avg_completed_tokens: 0,
          completed_prompt_tokens: 0,
          completed_completion_tokens: 0,
          completed_cached_tokens: 0,
          completed_total_tokens: 0,
          completed_requests: 0,
          first_observation_ms: 0,
          last_observation_ms: 0,
        },
      },
    });
    app.mount(container);
    await nextTick();

    // 持久化基准生效
    expect(container.textContent).toContain('周期基准用量');
    expect(container.textContent).toContain('5小时');
    expect(container.textContent).toContain('自然周');
    expect(container.textContent).toContain('自然月');

    // 主数字取持久化 5h 均值 1.25M
    expect(container.textContent).toContain('1.25M');
    // 周度持久化均值 7M
    expect(container.textContent).toContain('7M');

    app.unmount();
    document.body.removeChild(container);
  });

  it('renders realistic weekly baseline and four factors when completed_weekly_tokens is smaller than 5h', async () => {
    const container = document.createElement('div');
    document.body.appendChild(container);

    const keys: KeyView[] = [
      {
        id: 'acc-1',
        provider: 'antigravity',
        masked_key: 'ya29.***',
        state: 'active',
        priority: 1,
        weight: 10,
        usage: {
          account_tier: 'pro',
          confidence: 0.98,
          window_5h: {
            prompt_tokens: 100_000,
            completion_tokens: 5_000,
            cached_tokens: 20_000,
            total_tokens: 105_000,
            requests: 10,
          },
          window_weekly: {
            prompt_tokens: 5_000_000,
            completion_tokens: 200_000,
            cached_tokens: 1_000_000,
            total_tokens: 5_200_000,
            requests: 200,
          },
        },
      },
    ];

    const app = createApp(AntigravityPoolCard, {
      keys,
      keyTestResults: {},
      adminWriteEnabled: true,
      benchmark: {
        persisted_at_ms: 1_700_000_000_000,
        kind_5h: {
          observations: 10,
          avg_tokens: 2_000_000,
          prompt_tokens: 18_000_000,
          completion_tokens: 2_000_000,
          cached_tokens: 5_000_000,
          total_tokens: 20_000_000,
          requests: 100,
          completed_cycles: 0,
          avg_completed_tokens: 0,
          completed_prompt_tokens: 0,
          completed_completion_tokens: 0,
          completed_cached_tokens: 0,
          completed_total_tokens: 0,
          completed_requests: 0,
          first_observation_ms: 1_600_000_000_000,
          last_observation_ms: 1_700_000_000_000,
        },
        kind_weekly: {
          observations: 0,
          avg_tokens: 0,
          prompt_tokens: 0,
          completion_tokens: 0,
          cached_tokens: 0,
          total_tokens: 0,
          requests: 0,
          completed_cycles: 1,
          avg_completed_tokens: 300_000, // 早期碎片用量（小于 5h 的 2M）
          completed_prompt_tokens: 290_000,
          completed_completion_tokens: 10_000,
          completed_cached_tokens: 50_000,
          completed_total_tokens: 300_000,
          completed_requests: 12,
          first_observation_ms: 0,
          last_observation_ms: 0,
        },
        kind_monthly: {
          observations: 0,
          avg_tokens: 0,
          prompt_tokens: 0,
          completion_tokens: 0,
          cached_tokens: 0,
          total_tokens: 0,
          requests: 0,
          completed_cycles: 0,
          avg_completed_tokens: 0,
          completed_prompt_tokens: 0,
          completed_completion_tokens: 0,
          completed_cached_tokens: 0,
          completed_total_tokens: 0,
          completed_requests: 0,
          first_observation_ms: 0,
          last_observation_ms: 0,
        },
      },
    });
    app.mount(container);
    await nextTick();

    // 周用量应该回退到在册账号客观消耗（5.2M）而非小于 5h 的 300K
    expect(container.textContent).toContain('5.2M');
    // 缓存与调用次数也应从 weekly 窗口获取，不为空
    expect(container.textContent).toContain('1M'); // 1_000_000 cached
    expect(container.textContent).toContain('200次'); // 200 requests

    app.unmount();
    document.body.removeChild(container);
  });

  it('renders realistic cycle baseline without 4.33x fake multiplier and ignores unfinished slices', async () => {
    const container = document.createElement('div');
    document.body.appendChild(container);

    const keys: KeyView[] = [
      {
        id: 'acc-real-1',
        provider: 'antigravity',
        masked_key: 'ya29.***',
        state: 'active',
        priority: 1,
        weight: 10,
        usage: {
          account_tier: 'pro',
          // 仅有一个打满完成的 5h 周期
          completed_5h_stats: {
            count: 1,
            total_tokens: 1_200_000,
            avg_tokens: 1_200_000,
            prompt_tokens: 1_000_000,
            completion_tokens: 200_000,
            cached_tokens: 0,
            requests: 10,
          },
          // 具有预估周容量
          estimated_capacity_weekly: 5_000_000,
          window_5h: { total_tokens: 1_200_000, prompt_tokens: 1_000_000, completion_tokens: 200_000, cached_tokens: 0, requests: 10 },
          window_weekly: { total_tokens: 1_200_000, prompt_tokens: 1_000_000, completion_tokens: 200_000, cached_tokens: 0, requests: 10 },
          confidence: 0.9,
          // 未完成的月度窗口：无真实月度窗口数据
        },
      },
      {
        id: 'acc-real-2',
        provider: 'antigravity',
        masked_key: 'ya29.***',
        state: 'active',
        priority: 2,
        weight: 10,
        usage: {
          account_tier: 'pro',
          // 仅跑了 10,000 token 的未打满实时切片，未完成周期，无打满记录与推算容量
          window_5h: {
            total_tokens: 10_000,
            prompt_tokens: 8_000,
            completion_tokens: 2_000,
            cached_tokens: 0,
            requests: 2,
          },
          window_weekly: {
            total_tokens: 10_000,
            prompt_tokens: 8_000,
            completion_tokens: 2_000,
            cached_tokens: 0,
            requests: 2,
          },
          confidence: 0.1,
        },
      },
    ];

    const app = createApp(AntigravityPoolCard, {
      keys,
      keyTestResults: {},
      adminWriteEnabled: true,
    });
    app.mount(container);
    await nextTick();

    // 标题与副标题验证
    expect(container.textContent).toContain('周期基准用量');
    expect(container.textContent).toContain('多账号真实测定的单账号周期用量基准');
    expect(container.textContent).toContain('基于当前 2 个就绪账号剩余额度');

    // 5h 均值：未打满的 acc-real-2 不应拉低 acc-real-1 打满的 1.2M
    expect(container.textContent).toContain('1.2M');

    // 自然周均值：5M
    expect(container.textContent).toContain('5M');

    // 自然月均值：绝不出现 5M * 4.33 = 21.65M 的虚假数字，在无月度真实统计时应显示 '--'
    expect(container.textContent).not.toContain('21.65M');
    expect(container.textContent).not.toContain('22M');

    app.unmount();
    document.body.removeChild(container);
  });
});
