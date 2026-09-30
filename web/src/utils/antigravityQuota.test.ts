// @vitest-environment happy-dom
import { describe, it, expect } from 'vitest';
import {
  extractUnifiedGeminiQuota,
  isAntigravityScope,
  judgeKeyAvailability,
} from './antigravityQuota';
import type { KeyTestView } from '../types/admin';

describe('antigravityQuota shared source of truth', () => {
  it('aggregates quota_groups with min-wins and skips Claude family', () => {
    const res: KeyTestView = {
      success: true,
      latency_ms: 100,
      message: 'ok',
      quota_groups: [
        {
          display_name: 'Gemini',
          buckets: [
            { bucket_id: 'gemini-5h', window: '5h', remaining_fraction: 0.8 },
            { bucket_id: 'gemini-5h', window: '5h', remaining_fraction: 0.2 },
            { bucket_id: 'gemini-weekly', window: 'weekly', remaining_fraction: 0.6 },
          ],
        },
        {
          display_name: 'Claude 3.7 Sonnet',
          buckets: [{ bucket_id: 'claude-5h', window: '5h', remaining_fraction: 0.01 }],
        },
      ],
    };
    const u = extractUnifiedGeminiQuota(res);
    expect(u.h5?.fraction).toBe(0.2);
    expect(u.weekly?.fraction).toBe(0.6);
    expect(u.weeklySeen).toBe(true);
  });

  it('marks weekly as unknown instead of faking 100% when absent', () => {
    const res: KeyTestView = {
      success: true,
      latency_ms: 100,
      message: 'ok',
      quota: [{ model_id: 'gemini-2.5-flash', remaining_fraction: 0.5 }],
    };
    const u = extractUnifiedGeminiQuota(res);
    expect(u.h5?.fraction).toBe(0.5);
    expect(u.weekly).toBeNull();
    expect(u.weeklySeen).toBe(false);
  });

  it('judges availability from backend state, tolerating lock_busy', () => {
    expect(judgeKeyAvailability('active')).toBe('active');
    expect(judgeKeyAvailability('cooling_down')).toBe('cooling');
    expect(judgeKeyAvailability('disabled')).toBe('unavailable');
    // 本地周水位为 0 也不改判：冷却只跟随 state
    expect(
      judgeKeyAvailability('active', { success: true, latency_ms: 1, message: 'ok', quota: [] }),
    ).toBe('active');
    // 普通瞬态网络波动不把 active 降级为 unavailable（避免误伤可用性）
    expect(
      judgeKeyAvailability('active', { success: false, latency_ms: 1, message: 'temporary network timeout' }),
    ).toBe('active');
    // 凭据失效等硬错误判不可用，需人工处理
    expect(
      judgeKeyAvailability('active', { success: false, latency_ms: 1, message: 'invalid_grant: token expired', error_code: 'invalid_grant' }),
    ).toBe('unavailable');
    expect(
      judgeKeyAvailability('active', {
        success: false,
        latency_ms: 1,
        message: 'serialization lock held by another replica',
        error_code: 'lock_busy',
      }),
    ).toBe('active');
  });

  it('unifies antigravity scope via default_protocol first', () => {
    expect(isAntigravityScope('antigravity', 'antigravity')).toBe(true);
    expect(isAntigravityScope('renamed-provider', 'antigravity')).toBe(true);
    expect(isAntigravityScope('My-Antigravity', 'chat')).toBe(true);
    expect(isAntigravityScope('openai', 'chat')).toBe(false);
  });
});
