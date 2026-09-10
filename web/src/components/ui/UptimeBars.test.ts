// @vitest-environment happy-dom
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { createApp, nextTick, h } from 'vue';
import UptimeBars from './UptimeBars.vue';
import type { ConnectivitySlot } from '../../types/telemetry';

describe('UptimeBars Component', () => {
  let container: HTMLDivElement;

  beforeEach(() => {
    container = document.createElement('div');
    document.body.appendChild(container);
  });

  afterEach(() => {
    document.body.removeChild(container);
  });

  it('renders exactly 40 bars by default with empty state', async () => {
    const app = createApp({
      render: () => h(UptimeBars, { slots: [] }),
    });
    app.mount(container);
    await nextTick();

    const bars = container.querySelectorAll('[data-testid="uptime-bar"]');
    expect(bars.length).toBe(40);
    // All empty bars should have neutral/slate color
    expect(bars[0].className).toContain('bg-slate-400');
    app.unmount();
  });

  it('correctly maps 300ms and 1000ms latency thresholds to green/yellow/red colors', async () => {
    const slots: ConnectivitySlot[] = [
      { timestamp_ms: 1000, latency_ms: 120, status: 'ok' },        // < 300ms -> emerald
      { timestamp_ms: 2500, latency_ms: 450, status: 'degraded' },  // 300-1000ms -> amber
      { timestamp_ms: 4000, latency_ms: 1200, status: 'down' },     // >= 1000ms -> rose
      { timestamp_ms: 5500, latency_ms: undefined, status: 'empty' } // empty -> slate
    ];

    const app = createApp({
      render: () => h(UptimeBars, { slots, latestLatencyMs: 120 }),
    });
    app.mount(container);
    await nextTick();

    const bars = container.querySelectorAll('[data-testid="uptime-bar"]');
    expect(bars.length).toBe(40);

    // Find the non-empty rendered bars (the 4 custom slots placed at the end)
    const slotEmerald = container.querySelector('[data-status="ok"]');
    const slotAmber = container.querySelector('[data-status="degraded"]');
    const slotRose = container.querySelector('[data-status="down"]');

    expect(slotEmerald?.className).toContain('bg-emerald-500');
    expect(slotAmber?.className).toContain('bg-amber-400');
    expect(slotRose?.className).toContain('bg-rose-500');

    // Verify latest latency is displayed
    const latencyLabel = container.querySelector('[data-testid="latest-latency"]');
    expect(latencyLabel?.textContent).toContain('120.0 ms');

    app.unmount();
  });

  it('safely handles null or undefined latestLatencyMs without throwing TypeError', async () => {
    const app = createApp({
      render: () => h(UptimeBars, { slots: [], latestLatencyMs: null as unknown as number }),
    });
    // Should mount without throwing TypeError on null.toFixed
    expect(() => app.mount(container)).not.toThrow();
    await nextTick();

    const latencyLabel = container.querySelector('[data-testid="latest-latency"]');
    expect(latencyLabel?.textContent?.trim()).toBe('--');
    expect(latencyLabel?.className).toContain('text-slate-500');
    app.unmount();
  });

  it('renders exactly 24 bars when slotCount is 24, displaying call metrics in tooltip and 24h speed in t/s', async () => {
    const slots: ConnectivitySlot[] = Array.from({ length: 24 }, (_, i) => ({
      timestamp_ms: 1000 + i * 5000,
      latency_ms: i === 23 ? 90 : 80 + i,
      tps: 45 + i,
      status: i === 20 ? 'degraded' : i === 21 ? 'down' : 'ok',
    }));

    const app = createApp({
      render: () => h(UptimeBars, {
        slots,
        slotCount: 24,
        latestLatencyMs: 90,
        speed24h: 48.6,
      }),
    });
    app.mount(container);
    await nextTick();

    // Exactly 24 bars (5s/柱, 最近2分钟)
    const bars = container.querySelectorAll('[data-testid="uptime-bar"]');
    expect(bars.length).toBe(24);

    // Verify 24-bar width is narrow (w-1)
    expect(bars[0].className).toContain('w-1');

    // Tooltip includes status, latency, and speed in integer t/s
    const firstBarTitle = bars[0].getAttribute('title') || '';
    expect(firstBarTitle).toContain('80.0 ms');
    expect(firstBarTitle).toContain('45 t/s');
    expect(firstBarTitle).toContain('响应及时');

    // 24h speed badge in t/s is present and rounded to integer
    const speedBadge = container.querySelector('[data-testid="speed-24h"]');
    expect(speedBadge).not.toBeNull();
    expect(speedBadge?.textContent).toContain('24h');
    expect(speedBadge?.textContent).toContain('49 t/s');

    app.unmount();
  });
});
