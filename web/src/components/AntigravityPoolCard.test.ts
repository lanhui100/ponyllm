// @vitest-environment happy-dom
import { describe, it, expect, vi } from 'vitest';
import { createApp, nextTick } from 'vue';
import AntigravityPoolCard from './AntigravityPoolCard.vue';
import type { KeyView, KeyTestView } from '../types/admin';

describe('AntigravityPoolCard Component', () => {
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
    // Verify empty placeholder slots are rendered (80 total - 2 active = 78 empty)
    const emptySlots = container.querySelectorAll('[data-testid="slot-heatmap-empty"]');
    expect(emptySlots.length).toBe(78);

    const grid = container.querySelector('[data-testid="slot-heatmap-grid"]');
    expect(grid?.className).toContain('grid-rows-5');

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

    app.mount(container);
    await nextTick();

    // 未知周不再误判：active 账号必须计入就绪，而非“所有账号冷却中”
    expect(container.textContent).toContain('1/1 账号就绪');
    expect(container.textContent).not.toContain('所有账号冷却中');
    // 5h 取同系列最小值（与模型管理页取最小值语义一致）：0.0068*100=0.68→1%
    // 精确断言 testid，避免与可用率/周水位的 '100%' 混淆
    expect(container.querySelector('[data-testid="gemini-h5-percent"]')?.textContent).toContain('1%');
    expect(container.querySelector('[data-testid="gemini-weekly-percent"]')?.textContent).toContain('100%');
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

    // Click cell to open modal
    cell.click();
    await nextTick();

    const modal = document.querySelector('[data-testid="account-details-modal"]');
    expect(modal).not.toBeNull();
    expect(modal?.textContent).toContain('Pro 会员');
    expect(modal?.textContent).toContain('100,000');
    expect(modal?.textContent).toContain('500,000');

    app.unmount();
    document.body.removeChild(container);
  });
});
