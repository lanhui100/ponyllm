// @vitest-environment happy-dom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useTelemetry } from './useTelemetry';

describe('useTelemetry composable (WEB-02 telemetry dual-link & degradation)', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    window.localStorage?.clear();
    vi.useFakeTimers();
  });

  afterEach(() => {
    window.localStorage?.clear();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it('initializes with default status and offline/pending transport', () => {
    const { health, transport, isDown } = useTelemetry({ autoStart: false });
    expect(health.value).toBe('unknown');
    expect(transport.value).toBe('polling');
    expect(isDown.value).toBe(false);
  });

  it('updates metrics and history buffer on poll tick', async () => {
    const mockHealth = { status: 'ok', version: '0.1.0' };
    const mockMetrics = {
      total_requests: 100,
      successful_requests: 98,
      failed_requests: 2,
      total_failover: 0,
      prompt_tokens: 5000,
      completion_tokens: 2000,
      total_tokens: 7000,
      stream: {
        stream_count: 50,
        avg_ttft_ms: 120,
        avg_ttlb_ms: 800,
        avg_chunks: 10,
        total_stalls: 0,
        max_gap_ms: 15,
        avg_tps: 45.2,
        total_bytes: 12000,
        total_chunks: 500,
      },
    };
    const mockStream = {
      global: mockMetrics.stream,
      providers: {},
      dropped: 0,
    };

    globalThis.fetch = vi.fn().mockImplementation((url: string) => {
      if (url.includes('/health')) {
        return Promise.resolve(new Response(JSON.stringify(mockHealth), { status: 200 }));
      }
      if (url.includes('/metrics')) {
        return Promise.resolve(new Response(JSON.stringify(mockMetrics), { status: 200 }));
      }
      if (url.includes('/stream')) {
        return Promise.resolve(new Response(JSON.stringify(mockStream), { status: 200 }));
      }
      return Promise.reject(new Error('Unknown url'));
    });

    const { health, metrics, history, start, stop } = useTelemetry({ autoStart: false, pollingInterval: 1000 });
    await start();

    expect(health.value).toBe('ok');
    expect(metrics.value?.total_requests).toBe(100);
    expect(history.value.length).toBe(1);
    expect(history.value[0].qps).toBeGreaterThanOrEqual(0);

    // Advance timer for next tick
    await vi.advanceTimersByTimeAsync(1000);
    expect(history.value.length).toBe(2);

    stop();
  });

  it('handles DOWN status and marks isDown true', async () => {
    globalThis.fetch = vi.fn().mockImplementation((url: string) => {
      if (url.includes('/health')) {
        return Promise.resolve(new Response(JSON.stringify({ status: 'down' }), { status: 503 }));
      }
      return Promise.reject(new Error('Down'));
    });

    const { health, isDown, start, stop } = useTelemetry({ autoStart: false });
    await start();

    expect(health.value).toBe('down');
    expect(isDown.value).toBe(true);

    stop();
  });

  it('caps history buffer at maximum 20 data points (30s window)', async () => {
    globalThis.fetch = vi.fn().mockResolvedValue(new Response(JSON.stringify({ status: 'ok' }), { status: 200 }));
    const { history, start, stop } = useTelemetry({ autoStart: false, pollingInterval: 100 });
    await start();

    for (let i = 0; i < 25; i++) {
      await vi.advanceTimersByTimeAsync(100);
    }

    expect(history.value.length).toBeLessThanOrEqual(20);
    stop();
  });

  it('persists gateway slots in localStorage and restores them seamlessly across instances/refreshes', async () => {
    const mockHealth = { status: 'ok', version: '0.1.0' };
    globalThis.fetch = vi.fn().mockImplementation((url: string) => {
      if (url.includes('/health')) {
        return Promise.resolve(new Response(JSON.stringify(mockHealth), { status: 200 }));
      }
      return Promise.resolve(new Response(JSON.stringify({}), { status: 200 }));
    });

    // 1. First session records gateway slots
    const t1 = useTelemetry({ autoStart: false, pollingInterval: 1000 });
    await t1.start();
    expect(t1.gatewayUptimeBars.value.slots.length).toBe(1);
    await vi.advanceTimersByTimeAsync(1000);
    expect(t1.gatewayUptimeBars.value.slots.length).toBe(2);
    t1.stop();

    // Verify localStorage has persisted data
    const raw = window.localStorage.getItem('ponyllm_gateway_slots_v1');
    expect(raw).not.toBeNull();
    const parsed = JSON.parse(raw!);
    expect(parsed.slots.length).toBe(2);

    // 2. Simulating page refresh by creating a new composable instance
    const t2 = useTelemetry({ autoStart: false, pollingInterval: 1000 });
    // Without waiting for new fetch, initial state immediately has the 2 persisted slots
    expect(t2.gatewayUptimeBars.value.slots.length).toBe(2);

    // Continuing updates on next tick
    await t2.start();
    expect(t2.gatewayUptimeBars.value.slots.length).toBe(3);
    t2.stop();
  });
});
