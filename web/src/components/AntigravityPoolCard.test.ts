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
    expect(container.textContent).toContain('5小时滚动容量');
    expect(container.textContent).toContain('80%'); // Gemini 5h
    expect(container.textContent).toContain('60%'); // Claude 5h
    expect(container.textContent).toContain('3小时后重置');

    // 5. Verify Weekly rolling capacities
    expect(container.textContent).toContain('周度滚动容量');
    expect(container.textContent).toContain('90%'); // Gemini Weekly
    expect(container.textContent).toContain('75%'); // Claude Weekly

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
});
