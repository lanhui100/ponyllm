// @vitest-environment happy-dom
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { createApp, nextTick } from 'vue';
import MetricCards from './MetricCards.vue';
import ProviderMatrix from './ProviderMatrix.vue';

describe('Token Cache Hit Rate Calculation Accuracy', () => {
  let container: HTMLDivElement;

  beforeEach(() => {
    container = document.createElement('div');
    document.body.appendChild(container);
  });

  afterEach(() => {
    document.body.removeChild(container);
  });

  it('MetricCards calculates cache hit rate against prompt_tokens accurately without artificial 48% compression', async () => {
    // Typical realistic scenario: 100K prompt with 90K cached hits -> 90%
    const app = createApp(MetricCards, {
      metrics: {
        total_requests: 10,
        successful_requests: 10,
        failed_requests: 0,
        total_failover: 0,
        prompt_tokens: 100_000,
        completion_tokens: 5_000,
        cached_tokens: 90_000,
        total_tokens: 105_000,
      },
    });
    app.mount(container);
    await nextTick();

    const text = container.textContent || '';
    // In old buggy formula: 90000 / (100000 + 90000) = 47.3% -> 47%
    // In correct formula: 90000 / 100000 = 90%
    expect(text).toContain('(90%)');
    expect(text).not.toContain('(47%)');
    expect(text).not.toContain('(48%)');
    app.unmount();
  });

  it('ProviderMatrix calculates provider cache hit rate against prompt_tokens accurately', async () => {
    const app = createApp(ProviderMatrix, {
      providers: {
        antigravity: {
          provider: 'antigravity',
          prompt_tokens: 200_000,
          completion_tokens: 10_000,
          cached_tokens: 180_000,
          total_tokens: 210_000,
        },
      },
    });
    app.mount(container);
    await nextTick();

    const text = container.textContent || '';
    // 180000 / 200000 = 90%
    expect(text).toContain('90%');
    expect(text).not.toContain('47%');
    expect(text).not.toContain('48%');
    app.unmount();
  });
});

