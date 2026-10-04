import type { KeyTestView } from '../types/admin';

/** Claude/GPT/3P 家族：上游已不再下发其额度，解析时直接跳过，防止残留污染 Gemini 水位。 */
export function isClaudeFamilyToken(s: string): boolean {
  const v = (s || '').toLowerCase();
  return (
    v.includes('claude') ||
    v.includes('gpt') ||
    v.includes('3p') ||
    v.includes('sonnet') ||
    v.includes('opus')
  );
}

export interface UnifiedBucketQuota {
  /** 剩余比例 0~1；null = 上游未下发（未知），调用方必须渲染中性占位而非 0%/100%。 */
  fraction: number | null;
  timeUntilReset: string;
}

export interface UnifiedGeminiQuota {
  h5: UnifiedBucketQuota | null;
  weekly: UnifiedBucketQuota | null;
  /** 是否见过真实周窗口信号（quota_groups 周桶 / 平铺周标识）。 */
  weeklySeen: boolean;
}

function hintOf(bucket: { time_until_reset?: string | null; reset_time_beijing?: string | null }): string {
  return bucket.time_until_reset || bucket.reset_time_beijing || '已就绪';
}

function isWeeklyBucket(win: string, bId: string, bDesc: string, bDisp: string): boolean {
  return (
    win === 'weekly' ||
    bId.includes('week') ||
    bDesc.includes('week') ||
    bDisp.includes('周') ||
    bId.includes('7d')
  );
}

function is5hBucket(win: string, bId: string, bDesc: string, bDisp: string): boolean {
  return (
    win === '5h' ||
    win.includes('5') ||
    bId.includes('5h') ||
    bId.includes('hour') ||
    bDesc.includes('5') ||
    bDisp.includes('5小时') ||
    bDisp.includes('session')
  );
}

/**
 * Antigravity 配额解析唯一真源（Dashboard 矩阵与模型管理共用）。
 *
 * - quota_groups 优先：周桶末值胜出改"首次写入 wins + 取最小"，与平铺口径一致；
 *   非周非 5h 的未知桶归入 5h（首个），不再强制第二个进 weekly 伪造周数据。
 * - 平铺 quota：周标识取最小，非周取最小为 5h。
 * - 周缺席 => weekly=null + weeklySeen=false（未知），调用方渲染"未下发"占位。
 * - 无任何数据源 => { h5: null, weekly: null }，调用方按冷却/未探测渲染占位。
 */
export function extractUnifiedGeminiQuota(keyResult?: KeyTestView | null): UnifiedGeminiQuota {
  const res: UnifiedGeminiQuota = { h5: null, weekly: null, weeklySeen: false };
  if (!keyResult) return res;

  if (keyResult.quota_groups && keyResult.quota_groups.length > 0) {
    for (const group of keyResult.quota_groups) {
      if (isClaudeFamilyToken(group.display_name || '')) continue;
      for (const bucket of group.buckets || []) {
        const win = (bucket.window || '').toLowerCase();
        const bId = (bucket.bucket_id || '').toLowerCase();
        const bDesc = (bucket.description || '').toLowerCase();
        const bDisp = (bucket.display_name || '').toLowerCase();
        const fraction = bucket.remaining_fraction ?? 0;
        const q: UnifiedBucketQuota = { fraction, timeUntilReset: hintOf(bucket) };
        if (isWeeklyBucket(win, bId, bDesc, bDisp)) {
          res.weeklySeen = true;
          if (!res.weekly || fraction < (res.weekly.fraction ?? 1)) res.weekly = q;
        } else if (is5hBucket(win, bId, bDesc, bDisp) || !res.h5) {
          if (!res.h5 || fraction < (res.h5.fraction ?? 1)) res.h5 = q;
        } else {
          // 未知窗口且已有 5h：不伪造周数据，保持 weekly=null（未知）。
        }
      }
    }
    return res;
  }

  if (keyResult.quota && keyResult.quota.length > 0) {
    for (const q of keyResult.quota) {
      const mId = (q.model_id || '').toLowerCase();
      if (isClaudeFamilyToken(mId)) continue;
      const fraction = q.remaining_fraction ?? 0;
      const item: UnifiedBucketQuota = {
        fraction,
        timeUntilReset: q.time_until_reset || q.reset_time_beijing || '已就绪',
      };
      const isWeeklyModel =
        mId.includes('week') ||
        mId.includes('7d') ||
        (q.time_until_reset !== undefined &&
          q.time_until_reset !== null &&
          (q.time_until_reset.includes('天') || q.time_until_reset.includes('d')));
      if (isWeeklyModel) {
        res.weeklySeen = true;
        if (!res.weekly || fraction < (res.weekly.fraction ?? 1)) res.weekly = item;
      } else if (!res.h5 || fraction < (res.h5.fraction ?? 1)) {
        res.h5 = item;
      }
    }
  }
  return res;
}

/**
 * 可用判定唯一真源：以后端 KeyView.state 为准（与后端 entry.rs 优先级一致：
 * Disabled > CoolingDown > Active）。
 * - disabled => 不可用；cooling_down => 冷却（探测失败也不翻转，避免瞬态抖动）。
 * - active + 硬失败（凭据失效/需验证/违规等永久性信号）=> 不可用。
 * - active + 瞬态失败（lock_busy/超时/5xx/网络/配额摘要缺失）=> 仍按 state 就绪，
 *   失败只影响水位占位，不翻可用性。
 * - 无探测结果 => 不改判（未知），调用方渲染"等待刷新"而非冷却。
 */
export function isLockBusyResult(r?: KeyTestView | null): boolean {
  if (!r) return false;
  const code = (r.error_code || '').toLowerCase();
  const msg = (r.message || '').toLowerCase();
  return code.includes('lock_busy') || msg.includes('serialization lock') || msg.includes('held by another replica');
}

/** 探测失败是否为永久性（需人工介入）信号。瞬态失败一律返回 false。 */
export function isProbeHardFailure(r?: KeyTestView | null): boolean {
  if (!r || r.success !== false) return false;
  if (isLockBusyResult(r)) return false;
  const code = (r.error_code || '').toLowerCase();
  const msg = (r.message || '').toLowerCase();
  const combined = `${code} ${msg}`;
  return (
    combined.includes('invalid_grant') ||
    combined.includes('token has been expired') ||
    combined.includes('revoked') ||
    combined.includes('validation_required') ||
    combined.includes('verify your account') ||
    combined.includes('policy') ||
    combined.includes('terms of service') ||
    combined.includes('suspended') ||
    combined.includes('unauthorized') ||
    combined.includes('invalid key') ||
    combined.includes('invalid_api_key') ||
    // 上游资格受限（RESTRICTED_AGE / "not eligible for"）：硬信号，
    // 冻结数日、需上游状态变化才恢复——与 quota/rate-limit 软冷却区分。
    combined.includes('eligibility') ||
    combined.includes('not eligible') ||
    /\b401\b/.test(combined)
  );
}

export type UnifiedKeyAvailability = 'unavailable' | 'cooling' | 'active';

export function judgeKeyAvailability(
  state: string,
  testResult?: KeyTestView | null,
): UnifiedKeyAvailability {
  if (state === 'disabled') return 'unavailable';
  if (state === 'cooling_down') return 'cooling';
  if (isProbeHardFailure(testResult)) return 'unavailable';
  return 'active';
}

/**
 * 冷却倒计时共享逻辑（Dashboard 矩阵与模型管理共用，原两份逐行重复拷贝收敛于此）。
 * 快照语义：keys 数组每次全量替换时重拍 remaining/fetchedAt，本地按秒递减；
 * 到期由调用方的 1s tick 检测并触发单次对齐（防抖在调用方）。
 */
export interface CooldownSnapshot {
  remaining: number;
  fetchedAt: number;
}

export function recordCooldownSnapshots(
  snapshots: Map<string, CooldownSnapshot>,
  keys: { id: string; state: string; cooldown_remaining_secs?: number | null }[],
  now: number = Date.now(),
): void {
  for (const k of keys) {
    if (k.state === 'cooling_down' && k.cooldown_remaining_secs != null) {
      snapshots.set(k.id, { remaining: k.cooldown_remaining_secs, fetchedAt: now });
    } else {
      snapshots.delete(k.id);
    }
  }
}

export function cooldownRemainingSecsFrom(
  snapshots: Map<string, CooldownSnapshot>,
  k: { id: string; cooldown_remaining_secs?: number | null; cooldown_reset_at?: string | null },
  nowMs: number,
): number | null {
  const snapshot = snapshots.get(k.id);
  if (snapshot) {
    const elapsedSecs = Math.floor((nowMs - snapshot.fetchedAt) / 1000);
    return Math.max(0, snapshot.remaining - elapsedSecs);
  }
  if (k.cooldown_remaining_secs != null) {
    return Math.max(0, k.cooldown_remaining_secs);
  }
  if (k.cooldown_reset_at) {
    const resetMs = new Date(k.cooldown_reset_at).getTime();
    if (!Number.isNaN(resetMs)) {
      return Math.max(0, Math.floor((resetMs - nowMs) / 1000));
    }
  }
  return null;
}

export function formatCooldownDuration(secs: number | null): string {
  if (secs == null || secs <= 0) return '';
  const days = Math.floor(secs / 86400);
  const hours = Math.floor((secs % 86400) / 3600);
  const minutes = Math.floor((secs % 3600) / 60);
  if (days > 0) return `${days}天${hours}小时`;
  if (hours > 0) return `${hours}小时${minutes}分`;
  if (minutes > 0) return `${minutes}分`;
  return `${secs}秒`;
}
export function isAntigravityScope(providerName: string, defaultProtocol?: string | null): boolean {
  if ((defaultProtocol || '').toLowerCase() === 'antigravity') return true;
  return (providerName || '').toLowerCase().includes('antigravity');
}
