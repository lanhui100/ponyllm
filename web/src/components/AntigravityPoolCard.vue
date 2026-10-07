<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from 'vue';
import type { KeyView, KeyTestView, QuotaCycleBenchmarkView } from '../types/admin';
import Icons from './ui/Icons.vue';
import UiTooltip from './ui/UiTooltip.vue';
import UiButton from './ui/UiButton.vue';
import { formatTokenHuman } from '../utils/format';
import {
  extractUnifiedGeminiQuota,
  judgeKeyAvailability,
  recordCooldownSnapshots,
  cooldownRemainingSecsFrom,
  formatCooldownDuration,
  isLockBusyResult,
  isProbeHardFailure,
  type CooldownSnapshot,
} from '../utils/antigravityQuota';
import { isQuotaResultFresh } from '../composables/useAdminConfig';

const props = withDefaults(
  defineProps<{
    keys: KeyView[];
    keyTestResults?: Record<string, KeyTestView>;
    /** 池级跨账号跨周期持久化累计基准（后端快照归档，跨发布/账号增删不归零）。 */
    benchmark?: QuotaCycleBenchmarkView | null;
    isRefreshing?: boolean;
    adminWriteEnabled?: boolean;
  }>(),
  {
    keyTestResults: () => ({}),
    benchmark: null,
    isRefreshing: false,
    adminWriteEnabled: true,
  }
);

const emit = defineEmits<{
  (e: 'refresh-quotas'): void;
  (e: 'cooldown-expired'): void;
  (e: 'navigate-governance'): void;
}>();

onUnmounted(() => {
  if (timer) {
    clearInterval(timer);
    timer = null;
  }
  if (resizeObserver) {
    resizeObserver.disconnect();
    resizeObserver = null;
  }
});

// 客户端动态秒级时钟信号，驱动倒计时平滑递减
const nowMs = ref(Date.now());
let timer: ReturnType<typeof setInterval> | null = null;
const recordedSnapshots = new Map<string, CooldownSnapshot>();

watch(
  () => props.keys,
  (newKeys) => {
    recordCooldownSnapshots(recordedSnapshots, newKeys);
  },
  { immediate: true, deep: true }
);

function checkCooldownsAndTick() {
  nowMs.value = Date.now();
  let hasExpired = false;
  for (const k of props.keys) {
    if (k.state === 'cooling_down') {
      const remaining = cooldownRemainingSecs(k);
      if (remaining != null && remaining <= 0) {
        hasExpired = true;
      }
    }
  }
  if (hasExpired) {
    emit('cooldown-expired');
  }
}

onMounted(() => {
  timer = setInterval(checkCooldownsAndTick, 1000);
});

onUnmounted(() => {
  if (timer) {
    clearInterval(timer);
    timer = null;
  }
});

function cooldownRemainingSecs(k: KeyView): number | null {
  return cooldownRemainingSecsFrom(recordedSnapshots, k, nowMs.value);
}

// 统计信息
const totalAccounts = computed(() => props.keys.length);

function isKeyUnavailable(k: KeyView): boolean {
  return judgeKeyAvailability(k.state, props.keyTestResults[k.id]) === 'unavailable';
}

function isKeyCoolingDown(k: KeyView): boolean {
  // 唯一真源：后端 state。本地周水位不再参与冷却判定（交还后端探测回调），
  // 避免与模型管理页口径打架、无探测结果时误判冷却。
  return judgeKeyAvailability(k.state, props.keyTestResults[k.id]) === 'cooling';
}

const disabledKeys = computed(() => {
  return props.keys.filter((k) => isKeyUnavailable(k));
});

const coolingKeys = computed(() => {
  return props.keys.filter((k) => !isKeyUnavailable(k) && isKeyCoolingDown(k));
});

const activeKeys = computed(() => {
  return props.keys.filter((k) => !isKeyUnavailable(k) && !isKeyCoolingDown(k));
});

// 统计 Pro 账号与普通账号数量及容量画像
const accountTierSummary = computed(() => {
  let proCount = 0;
  let standardCount = 0;
  let calibratingCount = 0;
  for (const k of props.keys) {
    const usage = props.keyTestResults[k.id]?.usage || k.usage;
    const tier = usage?.account_tier;
    if (tier === 'pro') {
      proCount++;
    } else if (tier === 'standard') {
      standardCount++;
    } else {
      calibratingCount++;
    }
  }
  return { proCount, standardCount, calibratingCount };
});

const availabilityRate = computed(() => {
  if (totalAccounts.value === 0) return 0;
  return Math.round((activeKeys.value.length / totalAccounts.value) * 100);
});

// 最快恢复的账号及秒数
const nextRecovery = computed(() => {
  const cooling = coolingKeys.value;
  if (cooling.length === 0) return null;

  let minSecs = Infinity;
  let targetKey: KeyView | null = null;
  let fallbackHint = '';

  for (const k of cooling) {
    const secs = cooldownRemainingSecs(k);
    if (secs != null && secs > 0 && secs < minSecs) {
      minSecs = secs;
      targetKey = k;
    }
    if (!fallbackHint) {
      const testResult = props.keyTestResults[k.id];
      if (testResult && isQuotaResultFresh(k.id)) {
        const q = extractKeyQuota(k, testResult);
        if (q.gemini.weeklyFraction != null && q.gemini.weeklyFraction <= 0 && q.gemini.weeklyResetHint && q.gemini.weeklyResetHint !== '已就绪' && q.gemini.weeklyResetHint !== '冷却保护中' && q.gemini.weeklyResetHint !== '未下发') {
          fallbackHint = q.gemini.weeklyResetHint;
          targetKey = k;
        }
      }
    }
  }

  if (minSecs !== Infinity && targetKey) {
    return {
      secs: minSecs,
      label: formatCooldownDuration(minSecs),
      key: targetKey,
    };
  }

  if (fallbackHint && targetKey) {
    return {
      secs: null,
      label: fallbackHint,
      key: targetKey,
    };
  }

  return {
    secs: null,
    label: '冷却保护中',
    key: targetKey,
  };
});

interface ExtractedQuota {
  h5Fraction: number | null;
  h5ResetHint: string;
  /** null = 上游未下发（未知），渲染"未下发"占位，不参与聚合。 */
  weeklyFraction: number | null;
  weeklyResetHint: string;
}

// 额度解析走共享真源 extractUnifiedGeminiQuota；未知（null）调用方渲染占位。
function extractKeyQuota(k: KeyView, testResult?: KeyTestView): { gemini: ExtractedQuota } {
  const isCooling = k.state === 'cooling_down';

  // 无探测结果 / 过期缓存 / 失败且无数据：视为未知，渲染占位（不再伪装健康/耗尽）。
  if (!testResult || !isQuotaResultFresh(k.id)) {
    return {
      gemini: isCooling
        ? { h5Fraction: 0, h5ResetHint: '冷却保护中', weeklyFraction: 0, weeklyResetHint: '冷却保护中' }
        : { h5Fraction: null, h5ResetHint: '等待刷新', weeklyFraction: null, weeklyResetHint: '等待刷新' },
    };
  }
  const u = extractUnifiedGeminiQuota(testResult);
  if (!u.h5 && !u.weekly) {
    return {
      gemini: isCooling
        ? { h5Fraction: 0, h5ResetHint: '冷却保护中', weeklyFraction: 0, weeklyResetHint: '冷却保护中' }
        : { h5Fraction: null, h5ResetHint: '等待刷新', weeklyFraction: null, weeklyResetHint: '等待刷新' },
    };
  }
  return {
    gemini: {
      // 冷却态未知 h5 按 0 渲染（后端已判冷）；非冷却未知 h5 保持 null 显示"等待刷新"。
      h5Fraction: u.h5?.fraction ?? (isCooling ? 0 : null),
      h5ResetHint: u.h5?.timeUntilReset ?? (isCooling ? '冷却保护中' : '等待刷新'),
      weeklyFraction: u.weekly?.fraction ?? (isCooling ? 0 : null),
      weeklyResetHint: u.weekly?.timeUntilReset ?? (isCooling ? '冷却保护中' : '未下发'),
    },
  };
}

// 聚合有效可用账户在 5h 和周度窗口的水位百分比（未知水位不参与平均，避免把未知洗成 0%/100%）
const aggregatedQuotas = computed(() => {
  const active = activeKeys.value;
  if (active.length === 0) {
    return {
      gemini: { h5Percent: null as number | null, h5Hint: '所有账号冷却中', weeklyPercent: null as number | null, weeklyHint: '所有账号冷却中' },
    };
  }

  let geminiH5Sum = 0;
  let geminiH5Count = 0;
  let geminiWeeklySum = 0;
  let geminiWeeklyCount = 0;
  let geminiH5Hint = '';
  let geminiWeeklyHint = '';

  for (const k of active) {
    const q = extractKeyQuota(k, props.keyTestResults[k.id]);
    if (q.gemini.h5Fraction != null) {
      geminiH5Sum += q.gemini.h5Fraction;
      geminiH5Count += 1;
    }
    if (q.gemini.weeklyFraction != null) {
      geminiWeeklySum += q.gemini.weeklyFraction;
      geminiWeeklyCount += 1;
    }

    if (!geminiH5Hint && q.gemini.h5ResetHint !== '已就绪') geminiH5Hint = q.gemini.h5ResetHint;
    if (!geminiWeeklyHint && q.gemini.weeklyResetHint !== '已就绪' && q.gemini.weeklyResetHint !== '未下发') geminiWeeklyHint = q.gemini.weeklyResetHint;
  }

  return {
    gemini: {
      h5Percent: geminiH5Count > 0 ? Math.round((geminiH5Sum / geminiH5Count) * 100) : null,
      h5Hint: geminiH5Hint || (geminiH5Count > 0 ? '配额充足' : '等待刷新'),
      weeklyPercent: geminiWeeklyCount > 0 ? Math.round((geminiWeeklySum / geminiWeeklyCount) * 100) : null,
      weeklyHint: geminiWeeklyHint || (geminiWeeklyCount > 0 ? '配额充足' : '上游未下发周配额'),
    },
  };
});

// 针对各账号提取健康评分与热力方块状态。
// 色系：绿色单色阶表达可用额度（GitHub Pure Green Monochrome Scale）：
// 从账号存在但未拿到额度的极浅绿占位 (#e6f4ea)，到冷却状态的柔青薄荷绿，
// 再随可用额度由浅入深逐阶跃迁至充沛深翠绿 (#9be9a8 -> #40c463 -> #30a14e -> #216e39)。
// 语义色仅用于硬/异常状态，与绿色阶严格分开：琥珀 = 需安全验证；
// 玫红/深红 = 硬错误（凭据失效 / 资格受限冻结 / 违规停用 / 已禁用 / 探测异常）；
// 天蓝 = 跨副本锁同步；中灰 (#d0d7de) 仅用于"无账号"的预留空槽位。eligibility_frozen 专属玫红，
// 表示"上游资格受限、冻结数日、已跳过"，区别于软冷却的薄荷绿。
export type SlotHeatLevel =
  | 'cooling'
  | 'low'
  | 'medium'
  | 'high'
  | 'full'
  | 'validation_required'
  | 'auth_invalid'
  | 'policy_violation'
  | 'eligibility_frozen'
  | 'disabled'
  | 'probe_failed';

export interface HeatSlotItem {
  key: KeyView;
  level: SlotHeatLevel;
  heatClass: string;
  tooltipText: string;
  isCooling: boolean;
}

/** 展示侧截断上游原文（后端已截 2048 字符，UI 再收窄到 300 保护布局）。 */
function truncateErr(msg: string | null | undefined, max = 300): string {
  if (!msg) return '';
  return msg.length > max ? `${msg.slice(0, max)}…` : msg;
}

/** 色块状态的可读名称（读屏/aria 非颜色通道表达，插件色盲可区分）。 */
function levelAriaLabel(level: SlotHeatLevel): string {
  switch (level) {
    case 'eligibility_frozen':
      return '上游资格受限已冻结';
    case 'validation_required':
      return '需安全验证';
    case 'auth_invalid':
      return '凭据失效';
    case 'policy_violation':
      return '违规停用';
    case 'disabled':
      return '已禁用';
    case 'cooling':
      return '冷却中';
    case 'low':
    case 'medium':
    case 'high':
    case 'full':
      return '可用';
    case 'probe_failed':
      return '探测异常';
    default:
      return '未知';
  }
}

/**
 * 资格受限冻结格的单一渲染源（冷却分支与拨测分支共用）：
 * 红色错误块 + 解冻倒计时 + 上游原因 + "已冻结跳过"。同一账号两条路径
 * 文案一致，避免拨测后走探测分支丢失倒计时（review P1）。
 */
function frozenEligibilitySlot(
  k: KeyView,
  email: string,
  tierBadge: string,
  usageSummary: string,
): HeatSlotItem {
  const remaining = cooldownRemainingSecs(k);
  const label = formatCooldownDuration(remaining);
  const when = label
    ? `约 ${label} 后解冻`
    // 后端冻结时长是常量多日；避免把具体天数写死（后续若改配置即失配），
    // 且倒计时归零但后端尚未翻态时不得谎报"3 天后"。
    : '冻结数日后到期，等待上游状态变化';
  const errHint = k.error_message ? `\n原因: ${truncateErr(k.error_message)}` : '';
  return {
    key: k,
    level: 'eligibility_frozen',
    heatClass: 'bg-rose-600 hover:bg-rose-500',
    tooltipText: `账号: ${email}${tierBadge}\n状态: 上游资格受限（拒绝服务，已冻结跳过）\n冻结: ${when}${errHint}${usageSummary}`,
    isCooling: k.state === 'cooling_down',
  };
}

const slotMatrix = computed<HeatSlotItem[]>(() => {
  return props.keys.map((k) => {
    const testResult = props.keyTestResults[k.id];
    const quota = extractKeyQuota(k, testResult);
    // 冷却唯一真源：后端 state（与 activeKeys/模型管理页同表达式）。
    // 本地周水位不再参与判定；无探测/过期未知渲染"等待刷新"，不判冷。
    const isCooling = k.state === 'cooling_down';

    let email = k.id;
    if (email.startsWith('ag-')) {
      email = email.slice(3);
    }

    const usage = testResult?.usage || k.usage;
    let tierBadge = '';
    let usageSummary = '';
    if (usage) {
      const tierLabel = usage.account_tier === 'pro' ? '⭐ Pro 会员' : usage.account_tier === 'standard' ? '🔹 标准会员' : usage.account_tier === 'calibrating' ? '🔄 测算中' : usage.account_tier === 'unknown' ? '⏳ 待调用' : '⚪ 普通账号';
      tierBadge = ` [${tierLabel}]`;
      const h5Tokens = formatTokenHuman(usage.window_5h?.total_tokens ?? 0);
      const capTokens = usage.estimated_capacity_5h ? ` / 额度 ~${formatTokenHuman(usage.estimated_capacity_5h)}` : '';
      const wTokens = formatTokenHuman(usage.window_weekly?.total_tokens ?? 0);
      const cacheRatio = (usage.window_5h?.total_tokens ?? 0) > 0 && (usage.window_5h?.cached_tokens ?? 0) > 0
        ? ` (缓存命中 ${formatTokenHuman(usage.window_5h.cached_tokens)})`
        : '';
      usageSummary = `\n5小时已用: ${h5Tokens} (${usage.window_5h?.requests ?? 0}次)${capTokens}${cacheRatio}\n本周累计: ${wTokens} (${usage.window_weekly?.requests ?? 0}次)`;
    }

    if (k.state === 'disabled') {
      const reason = k.disabled_reason?.toLowerCase() || '';
      const reasonHint = k.disabled_reason ? `\n原因: ${k.disabled_reason}` : '';
      if (reason.includes('validation_required') || reason.includes('verify your account')) {
        return {
          key: k,
          level: 'validation_required',
          heatClass: 'bg-amber-500 hover:bg-amber-400 ring-1 ring-amber-400/60',
          tooltipText: `账号: ${email}${tierBadge}\n状态: 需安全验证 (Google安全拦截)${reasonHint}${usageSummary}`,
          isCooling: false,
        };
      }
      if (reason.includes('invalid_grant') || reason.includes('token has been expired') || reason.includes('revoked')) {
        return {
          key: k,
          level: 'auth_invalid',
          heatClass: 'bg-rose-500 hover:bg-rose-400',
          tooltipText: `账号: ${email}${tierBadge}\n状态: 授权凭据失效 (invalid_grant)${reasonHint}${usageSummary}`,
          isCooling: false,
        };
      }
      if (reason.includes('policy') || reason.includes('terms of service') || reason.includes('suspended')) {
        return {
          key: k,
          level: 'policy_violation',
          heatClass: 'bg-rose-800 hover:bg-rose-700',
          tooltipText: `账号: ${email}${tierBadge}\n状态: 违规停用 (PolicyViolation)${reasonHint}${usageSummary}`,
          isCooling: false,
        };
      }
      return {
        key: k,
        level: 'disabled',
        heatClass: 'bg-rose-600/90 hover:bg-rose-500',
        tooltipText: `账号: ${email}${tierBadge}\n状态: 已禁用 (不分配流量)${reasonHint}${usageSummary}`,
        isCooling: false,
      };
    }

    // 探测硬失败（凭据失效/需验证）：优先展现严重状态
    if (testResult && testResult.success === false) {
      const errMsg = (testResult.message || '').toLowerCase();
      const errCode = (testResult.error_code || '').toLowerCase();
      if (errCode.includes('validation') || errMsg.includes('validation_required') || errMsg.includes('verify your account')) {
        return {
          key: k,
          level: 'validation_required',
          heatClass: 'bg-amber-500 hover:bg-amber-400 ring-1 ring-amber-400/60',
          tooltipText: `账号: ${email}${tierBadge}\n状态: 需安全验证 (Google安全拦截)\n提示: ${testResult.message}${usageSummary}`,
          isCooling: false,
        };
      }
      if (errCode === 'invalid_grant' || errMsg.includes('invalid_grant') || errMsg.includes('token has been expired') || errMsg.includes('revoked')) {
        return {
          key: k,
          level: 'auth_invalid',
          heatClass: 'bg-rose-500 hover:bg-rose-400',
          tooltipText: `账号: ${email}${tierBadge}\n状态: 授权凭据失效 (invalid_grant)\n提示: ${testResult.message}${usageSummary}`,
          isCooling: false,
        };
      }
      if (errCode.includes('policy') || errMsg.includes('terms of service') || errMsg.includes('suspended')) {
        return {
          key: k,
          level: 'policy_violation',
          heatClass: 'bg-rose-800 hover:bg-rose-700',
          tooltipText: `账号: ${email}${tierBadge}\n状态: 违规停用 (PolicyViolation)\n提示: ${testResult.message}${usageSummary}`,
          isCooling: false,
        };
      }
      if (errCode.includes('eligibility') || errMsg.includes('not eligible for')) {
        // 拨测命中资格 403（后端已同请求内冻结该账号）。
        // - 后端 state 已确认冷却+资格 → 完整文案（含解冻倒计时，与冷却分支
        //   同一渲染源），避免探测分支丢失"已冻结跳过/倒计时"（review P1）。
        // - 仅探针证据而 state 仍是 active → 陈旧/矛盾数据：可能是冻结已
        //   过期但 localStorage 结果未失效（TTL 6h），不硬判红（review P2）。
        // - 其余（如后端仍未来得及刷新 state）→ 红色无倒计时，附探针证据。
        if (k.state === 'cooling_down' && k.cooldown_reason === 'eligibility') {
          return frozenEligibilitySlot(k, email, tierBadge, usageSummary);
        }
        if (k.state !== 'active') {
          return {
            key: k,
            level: 'eligibility_frozen',
            heatClass: 'bg-rose-600 hover:bg-rose-500',
            tooltipText: `账号: ${email}${tierBadge}\n状态: 上游资格受限（拒绝服务）\n提示: ${truncateErr(testResult.message)}${usageSummary}`,
            isCooling: false,
          };
        }
      }
    }

    // 冷却状态优先于瞬态网络/探测失败：长冻结的资格类账号显示红色错误，
    // 其余保持薄荷绿冷却保护。
    if (isCooling) {
      if (k.cooldown_reason === 'eligibility') {
        // 上游资格类 403（如 Gemini Code Assist "not eligible"）：账号被冻结数日。
        // 红色错误块 + 原因 + 解冻时间，与软冷却（薄荷绿）区分开。
        return frozenEligibilitySlot(k, email, tierBadge, usageSummary);
      }
      const remaining = cooldownRemainingSecs(k);
      const label = formatCooldownDuration(remaining);
      const hint = label
        ? `预计 ${label} 后解冻`
        : (quota.gemini.weeklyResetHint !== '冷却保护中' && quota.gemini.weeklyResetHint !== '已就绪' && quota.gemini.weeklyResetHint !== '未下发' && quota.gemini.weeklyResetHint !== '等待刷新'
          ? `预计 ${quota.gemini.weeklyResetHint} 解冻`
          : '等待解冻');
      return {
        key: k,
        level: 'cooling',
        // 纯绿色阶体系中的冷冻态：清晰可见的柔青薄荷绿（加深，对比鲜明）
        heatClass: 'bg-[#a3e4a8]',
        tooltipText: `账号: ${email}${tierBadge}\n状态: 冷却保护中\n重置: ${hint}${usageSummary}`,
        isCooling: true,
      };
    }

    if (testResult && testResult.success === false) {
      const errMsg = (testResult.message || '').toLowerCase();
      const errCode = (testResult.error_code || '').toLowerCase();
      if (errCode.includes('lock_busy') || errMsg.includes('serialization lock') || errMsg.includes('held by another replica')) {
        return {
          key: k,
          level: 'low',
          heatClass: 'bg-sky-400/80 hover:bg-sky-300 ring-1 ring-sky-400/50',
          tooltipText: `账号: ${email}${tierBadge}\n状态: 跨节点锁同步中 (等待另一副本刷新)\n提示: ${testResult.message}${usageSummary}`,
          isCooling: false,
        };
      }
      return {
        key: k,
        level: 'probe_failed',
        heatClass: 'bg-rose-400/80 hover:bg-rose-300',
        tooltipText: `账号: ${email}${tierBadge}\n状态: 探测异常\n提示: ${testResult.message}${usageSummary}`,
        isCooling: false,
      };
    }

    if (!testResult || !isQuotaResultFresh(k.id)) {
      const staleNote = usageSummary ? '\n(历史快照)' : '';
      return {
        key: k,
        level: 'low',
        // 账号存在但尚未探测/缓存过期/未持久化：极浅绿占位，表示"此槽位有账号，仅未拿到额度"，
        // 与"无账号空槽位"的灰阶 (#d0d7de) 语义严格区分（ADR 2026-10-07-pool-matrix-slot-presence）。
        heatClass: 'bg-[#e6f4ea] hover:bg-[#d3edda]',
        tooltipText: `账号: ${email}${tierBadge}\n状态: 等待刷新配额${staleNote}${usageSummary}`,
        isCooling: false,
      };
    }

    // 以 Gemini 配额为核心判断等级（兼顾 5h 即时爆发余量与周度余量）；未知按"等待刷新"极浅绿占位
    const g5hFraction = quota.gemini.h5Fraction;
    if (g5hFraction == null) {
      return {
        key: k,
        level: 'low',
        heatClass: 'bg-[#e6f4ea] hover:bg-[#d3edda]',
        tooltipText: `账号: ${email}${tierBadge}\n状态: 等待刷新配额${usageSummary}`,
        isCooling: false,
      };
    }
    const g5h = Math.round(g5hFraction * 100);
    const gWeekly = quota.gemini.weeklyFraction == null ? '--' : `${Math.round(quota.gemini.weeklyFraction * 100)}%`;

    const baseTooltip = `账号: ${email}${tierBadge}\nGemini: 5h余量 ${g5h}% · 周余量 ${gWeekly}${usageSummary}`;

    if (g5hFraction >= 0.75) {
      return {
        key: k,
        level: 'full',
        // 绿阶最高级 (≥75%): 浓郁高饱和深翠绿
        heatClass: 'bg-[#196127]',
        tooltipText: `${baseTooltip}\n状态: 额度充沛`,
        isCooling: false,
      };
    } else if (g5hFraction >= 0.45) {
      return {
        key: k,
        level: 'high',
        // 绿阶第3级 (45%~75%): 茂盛纯正经典绿
        heatClass: 'bg-[#239a3b]',
        tooltipText: `${baseTooltip}\n状态: 额度良好`,
        isCooling: false,
      };
    } else if (g5hFraction >= 0.15) {
      return {
        key: k,
        level: 'medium',
        // 绿阶第2级 (15%~45%): 醒目草绿 (加深清晰度)
        heatClass: 'bg-[#3cc15e]',
        tooltipText: `${baseTooltip}\n状态: 额度中等`,
        isCooling: false,
      };
    } else {
      return {
        key: k,
        level: 'low',
        // 绿阶第1级 (<15%): 清新明朗浅绿 (提高辨识度，不发白)
        heatClass: 'bg-[#7bc96f]',
        tooltipText: `${baseTooltip}\n状态: 额度偏低（Gemini 5h即将耗尽）`,
        isCooling: false,
      };
    }
  });
});

// 动态自适应列数：彻底去除 justify-between 的强行拉伸，确保方块横向间距与纵向间距完全由 gap-[3px] 控制（等距对齐）
// 在真实浏览器中根据容器宽度自动填满整行；在无宽度的测试环境下回退到基准 16 列
const gridContainerRef = ref<HTMLElement | null>(null);
const containerWidth = ref(0);
let resizeObserver: ResizeObserver | null = null;

const totalColumns = computed(() => {
  if (containerWidth.value <= 0) return 16;
  // 每个方块 14px (w-3.5)，间距 3px (gap-[3px])
  // 外层增加 p-1 (左右各 4px)，计算可用宽度：width - 8
  const availableWidth = Math.max(0, containerWidth.value - 8);
  const cols = Math.floor((availableWidth + 3) / 17);
  return Math.max(10, cols);
});

// 每 5 行为一组，支持垂直排列多组（当前展示 3 组）
const ROWS_PER_GROUP = 5;
const GROUP_COUNT = 3;
const slotsPerGroup = computed(() => totalColumns.value * ROWS_PER_GROUP);
const totalSlots = computed(() => slotsPerGroup.value * GROUP_COUNT);

interface MatrixGroup {
  items: HeatSlotItem[];
  emptyCount: number;
}

const matrixGroups = computed<MatrixGroup[]>(() => {
  const allSlots = slotMatrix.value;
  const perGroup = slotsPerGroup.value;
  const groups: MatrixGroup[] = [];

  for (let g = 0; g < GROUP_COUNT; g++) {
    const start = g * perGroup;
    const end = start + perGroup;
    const groupItems = allSlots.slice(start, Math.min(allSlots.length, end));
    const emptyCount = Math.max(0, perGroup - groupItems.length);
    groups.push({
      items: groupItems,
      emptyCount,
    });
  }

  return groups;
});

// 选中的账号用于展示单账号详细额度画像
const selectedKeyId = ref<string | null>(null);

function handleKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape' && selectedKeyId.value) {
    selectedKeyId.value = null;
  }
}

watch(selectedKeyId, (val) => {
  if (typeof window !== 'undefined') {
    if (val) {
      window.addEventListener('keydown', handleKeydown);
    } else {
      window.removeEventListener('keydown', handleKeydown);
    }
  }
});

function selectKeyForDetail(keyId: string) {
  if (selectedKeyId.value === keyId) {
    selectedKeyId.value = null;
  } else {
    selectedKeyId.value = keyId;
  }
}

const selectedKeyData = computed(() => {
  if (!selectedKeyId.value) return null;
  const k = props.keys.find((item) => item.id === selectedKeyId.value);
  if (!k) return null;
  const testRes = props.keyTestResults[k.id];
  const usage = testRes?.usage || k.usage;
  const quota = extractKeyQuota(k, testRes);
  return {
    key: k,
    testRes,
    usage,
    quota,
  };
});

onMounted(() => {
  if (typeof ResizeObserver !== 'undefined' && gridContainerRef.value) {
    resizeObserver = new ResizeObserver((entries) => {
      for (const entry of entries) {
        if (entry.contentRect.width > 0) {
          containerWidth.value = entry.contentRect.width;
        }
      }
    });
    resizeObserver.observe(gridContainerRef.value);
  }
});

onUnmounted(() => {
  if (resizeObserver) {
    resizeObserver.disconnect();
    resizeObserver = null;
  }
  if (typeof window !== 'undefined') {
    window.removeEventListener('keydown', handleKeydown);
  }
});

// 加权统计客观真实的完整周期实际消耗与四要素画像
const factualCycleSummary = computed(() => {
  let count5h = 0;
  let totalTokens5h = 0;
  let proCount5h = 0;
  let proTokens5h = 0;
  let standardCount5h = 0;
  let standardTokens5h = 0;

  let totalPrompt5h = 0;
  let totalComp5h = 0;
  let totalCached5h = 0;
  let totalRequests5h = 0;

  let totalTokensWeekly = 0;
  let weeklyAccounts = 0;
  let totalWeeklyCapacityEstimated = 0;
  let totalPromptWeekly = 0;
  let totalCompWeekly = 0;
  let totalCachedWeekly = 0;
  let totalRequestsWeekly = 0;

  let totalTokensMonthly = 0;
  let monthlyAccounts = 0;
  let totalPromptMonthly = 0;
  let totalCompMonthly = 0;
  let totalCachedMonthly = 0;
  let totalRequestsMonthly = 0;

  for (const k of props.keys) {
    const usage = props.keyTestResults[k.id]?.usage || k.usage;
    if (!usage) continue;

    // 1. 客观完整的 5h 周期历史统计（打满实测归档）
    if (usage.completed_5h_stats && usage.completed_5h_stats.count > 0) {
      count5h += usage.completed_5h_stats.count;
      totalTokens5h += usage.completed_5h_stats.total_tokens;
      totalPrompt5h += usage.completed_5h_stats.prompt_tokens ?? usage.window_5h?.prompt_tokens ?? 0;
      totalComp5h += usage.completed_5h_stats.completion_tokens ?? usage.window_5h?.completion_tokens ?? 0;
      totalCached5h += usage.completed_5h_stats.cached_tokens ?? usage.window_5h?.cached_tokens ?? 0;
      totalRequests5h += usage.completed_5h_stats.requests ?? usage.window_5h?.requests ?? 0;

      if (usage.account_tier === 'pro') {
        proCount5h += usage.completed_5h_stats.count;
        proTokens5h += usage.completed_5h_stats.total_tokens;
      } else if (usage.account_tier === 'standard' || usage.account_tier === 'free') {
        standardCount5h += usage.completed_5h_stats.count;
        standardTokens5h += usage.completed_5h_stats.total_tokens;
      }
    } else if (usage.estimated_capacity_5h && usage.estimated_capacity_5h > 0) {
      // 动态斜率推测的容量
      totalTokens5h += usage.estimated_capacity_5h;
      count5h += 1;
      totalPrompt5h += usage.window_5h?.prompt_tokens ?? 0;
      totalComp5h += usage.window_5h?.completion_tokens ?? 0;
      totalCached5h += usage.window_5h?.cached_tokens ?? 0;
      totalRequests5h += usage.window_5h?.requests ?? 0;

      if (usage.account_tier === 'pro') {
        proCount5h += 1;
        proTokens5h += usage.estimated_capacity_5h;
      } else if (usage.account_tier === 'standard' || usage.account_tier === 'free') {
        standardCount5h += 1;
        standardTokens5h += usage.estimated_capacity_5h;
      }
    }
    // 未打满且未推算容量的实时切片，不作为单账号完整周期的容量基准分母，避免拉低真实额度

    // 2. 周度额度反推与消耗
    if (usage.estimated_capacity_weekly && usage.estimated_capacity_weekly > 0) {
      totalWeeklyCapacityEstimated += usage.estimated_capacity_weekly;
      weeklyAccounts += 1;
      totalPromptWeekly += usage.window_weekly?.prompt_tokens ?? 0;
      totalCompWeekly += usage.window_weekly?.completion_tokens ?? 0;
      totalCachedWeekly += usage.window_weekly?.cached_tokens ?? 0;
      totalRequestsWeekly += usage.window_weekly?.requests ?? 0;
    }

    // 3. 月度客观消耗
    if (usage.window_monthly && usage.window_monthly.total_tokens > 0) {
      totalTokensMonthly += usage.window_monthly.total_tokens;
      monthlyAccounts += 1;
      totalPromptMonthly += usage.window_monthly.prompt_tokens;
      totalCompMonthly += usage.window_monthly.completion_tokens;
      totalCachedMonthly += usage.window_monthly.cached_tokens;
      totalRequestsMonthly += usage.window_monthly.requests;
    }
  }

  const liveAvg5h = count5h > 0 ? Math.round(totalTokens5h / count5h) : 0;
  const proAvg5h = proCount5h > 0 ? Math.round(proTokens5h / proCount5h) : 0;
  const standardAvg5h = standardCount5h > 0 ? Math.round(standardTokens5h / standardCount5h) : 0;
  const liveAvgPrompt5h = count5h > 0 ? Math.round(totalPrompt5h / count5h) : 0;
  const liveAvgComp5h = count5h > 0 ? Math.round(totalComp5h / count5h) : 0;
  const liveAvgCached5h = count5h > 0 ? Math.round(totalCached5h / count5h) : 0;
  const liveAvgRequests5h = count5h > 0 ? Math.round(totalRequests5h / count5h) : 0;

  const liveAvgWeekly = weeklyAccounts > 0
    ? Math.round((totalWeeklyCapacityEstimated > 0 ? totalWeeklyCapacityEstimated : totalTokensWeekly) / weeklyAccounts)
    : 0;
  const liveAvgPromptWeekly = weeklyAccounts > 0 ? Math.round(totalPromptWeekly / weeklyAccounts) : 0;
  const liveAvgCompWeekly = weeklyAccounts > 0 ? Math.round(totalCompWeekly / weeklyAccounts) : 0;
  const liveAvgCachedWeekly = weeklyAccounts > 0 ? Math.round(totalCachedWeekly / weeklyAccounts) : 0;
  const liveAvgRequestsWeekly = weeklyAccounts > 0 ? Math.round(totalRequestsWeekly / weeklyAccounts) : 0;

  const liveAvgMonthly = monthlyAccounts > 0 ? Math.round(totalTokensMonthly / monthlyAccounts) : 0;
  const liveAvgPromptMonthly = monthlyAccounts > 0 ? Math.round(totalPromptMonthly / monthlyAccounts) : 0;
  const liveAvgCompMonthly = monthlyAccounts > 0 ? Math.round(totalCompMonthly / monthlyAccounts) : 0;
  const liveAvgCachedMonthly = monthlyAccounts > 0 ? Math.round(totalCachedMonthly / monthlyAccounts) : 0;
  const liveAvgRequestsMonthly = monthlyAccounts > 0 ? Math.round(totalRequestsMonthly / monthlyAccounts) : 0;

  // ---- 池级跨账号跨周期持久化累计基准（后端快照归档，跨发布/账号增删不归零）----
  // 只要有持久化观测（含打满周期），即优先生效；否则回退到上面实时在册账号计算。
  // `kind_*` 可缺省（旧后端/降级响应），全程 null 安全。
  const b = props.benchmark;
  const persisted5h =
    !!b &&
    ((b.kind_5h?.observations ?? 0) > 0 || (b.kind_5h?.completed_cycles ?? 0) > 0)
      ? b.kind_5h
      : null;
  const persistedWeekly =
    !!b &&
    ((b.kind_weekly?.observations ?? 0) > 0 || (b.kind_weekly?.completed_cycles ?? 0) > 0)
      ? b.kind_weekly
      : null;
  const persistedMonthly =
    !!b &&
    ((b.kind_monthly?.observations ?? 0) > 0 || (b.kind_monthly?.completed_cycles ?? 0) > 0)
      ? b.kind_monthly
      : null;
  const persistedAtMs = b?.persisted_at_ms ?? 0;
  const usePersisted = !!(persisted5h || persistedWeekly || persistedMonthly);

  const avg5h =
    persisted5h && persisted5h.observations > 0
      ? persisted5h.avg_tokens
      : persisted5h && persisted5h.completed_cycles > 0
        ? persisted5h.avg_completed_tokens
        : liveAvg5h;
  const completedCyclesCount = persisted5h
    ? persisted5h.completed_cycles + (persistedWeekly?.completed_cycles ?? 0)
    : count5h;
  const avgPrompt5h = persisted5h && persisted5h.observations > 0
    ? Math.round(persisted5h.prompt_tokens / persisted5h.observations)
    : liveAvgPrompt5h;
  const avgComp5h = persisted5h && persisted5h.observations > 0
    ? Math.round(persisted5h.completion_tokens / persisted5h.observations)
    : liveAvgComp5h;
  const avgCached5h = persisted5h && persisted5h.observations > 0
    ? Math.round(persisted5h.cached_tokens / persisted5h.observations)
    : liveAvgCached5h;
  const avgRequests5h = persisted5h && persisted5h.observations > 0
    ? Math.round(persisted5h.requests / persisted5h.observations)
    : liveAvgRequests5h;

  const avgWeekly =
    persistedWeekly && persistedWeekly.observations > 0
      ? persistedWeekly.avg_tokens
      : persistedWeekly && persistedWeekly.completed_cycles > 0
        ? persistedWeekly.avg_completed_tokens
        : liveAvgWeekly;
  const avgPromptWeekly = persistedWeekly && persistedWeekly.observations > 0
    ? Math.round(persistedWeekly.prompt_tokens / persistedWeekly.observations)
    : liveAvgPromptWeekly;
  const avgCompWeekly = persistedWeekly && persistedWeekly.observations > 0
    ? Math.round(persistedWeekly.completion_tokens / persistedWeekly.observations)
    : liveAvgCompWeekly;
  const avgCachedWeekly = persistedWeekly && persistedWeekly.observations > 0
    ? Math.round(persistedWeekly.cached_tokens / persistedWeekly.observations)
    : liveAvgCachedWeekly;
  const avgRequestsWeekly = persistedWeekly && persistedWeekly.observations > 0
    ? Math.round(persistedWeekly.requests / persistedWeekly.observations)
    : liveAvgRequestsWeekly;

  const avgMonthly =
    persistedMonthly && persistedMonthly.observations > 0
      ? persistedMonthly.avg_tokens
      : persistedMonthly && persistedMonthly.completed_cycles > 0
        ? persistedMonthly.avg_completed_tokens
        : liveAvgMonthly;
  const avgPromptMonthly = persistedMonthly && persistedMonthly.observations > 0
    ? Math.round(persistedMonthly.prompt_tokens / persistedMonthly.observations)
    : liveAvgPromptMonthly;
  const avgCompMonthly = persistedMonthly && persistedMonthly.observations > 0
    ? Math.round(persistedMonthly.completion_tokens / persistedMonthly.observations)
    : liveAvgCompMonthly;
  const avgCachedMonthly = persistedMonthly && persistedMonthly.observations > 0
    ? Math.round(persistedMonthly.cached_tokens / persistedMonthly.observations)
    : liveAvgCachedMonthly;
  const avgRequestsMonthly = persistedMonthly && persistedMonthly.observations > 0
    ? Math.round(persistedMonthly.requests / persistedMonthly.observations)
    : liveAvgRequestsMonthly;

  const persistedObservations =
    (persisted5h?.observations ?? 0) +
    (persistedWeekly?.observations ?? 0) +
    (persistedMonthly?.observations ?? 0);

  return {
    avg5h,
    proAvg5h,
    standardAvg5h,
    avgPrompt5h,
    avgComp5h,
    avgCached5h,
    avgRequests5h,
    avgWeekly,
    avgPromptWeekly,
    avgCompWeekly,
    avgCachedWeekly,
    avgRequestsWeekly,
    avgMonthly,
    avgPromptMonthly,
    avgCompMonthly,
    avgCachedMonthly,
    avgRequestsMonthly,
    completedCyclesCount,
    isEstimatedWeekly: totalWeeklyCapacityEstimated > 0,
    // 持久化累计基准信息（用于标注，非数值来源时回退实时计算）。
    usePersisted,
    persistedObservations,
    persistedCompletedCycles: completedCyclesCount,
    persistedAtMs,
  };
});

function getProgressColor(percent: number | null): { bar: string; text: string; bg: string } {
  if (percent == null) {
    return { bar: 'bg-slate-300', text: 'text-slate-400', bg: 'bg-slate-50' };
  }
  if (percent > 30) {
    return { bar: 'bg-emerald-500', text: 'text-emerald-700', bg: 'bg-emerald-50' };
  } else if (percent > 10) {
    return { bar: 'bg-amber-500', text: 'text-amber-700', bg: 'bg-amber-50' };
  } else {
    return { bar: 'bg-rose-500', text: 'text-rose-700', bg: 'bg-rose-50' };
  }
}

function formatWaterPercent(percent: number | null): string {
  return percent == null ? '--' : `${percent}%`;
}

function waterBarWidth(percent: number | null): string {
  return percent == null ? '0%' : `${percent}%`;
}
</script>

<template>
  <div
    v-if="totalAccounts > 0"
    class="swiss-card p-4 bg-white/45 backdrop-blur-xs transition-all duration-200 border border-white/40 shadow-xs mb-6"
    :class="{ 'border-rose-200/80 bg-rose-50/20': activeKeys.length === 0 }"
  >
    <!-- 卡片头部 -->
    <div class="flex flex-wrap items-center justify-between gap-3 pb-3 mb-3 border-b border-slate-200/60">
      <div class="flex items-center gap-2.5">
        <div class="flex items-center justify-center text-slate-800">
          <Icons name="zap" size="16" />
        </div>
        <div>
          <h2 class="text-base font-bold text-slate-800 tracking-tight">Antigravity 算力池</h2>
        </div>
      </div>

      <div class="flex items-center gap-2">
        <UiTooltip :content="isRefreshing ? '正在同步刷新配额...' : '探测并刷新所有账号配额用量'">
          <UiButton
            variant="ghost"
            size="sm"
            :disabled="!adminWriteEnabled || isRefreshing"
            class="text-amber-600 hover:text-amber-700 hover:bg-amber-50/60 text-xs font-medium px-2.5 py-1.5 h-auto"
            data-testid="refresh-antigravity-pool-btn"
            @click="emit('refresh-quotas')"
          >
            <Icons
              name="refresh"
              size="13"
              class="mr-1"
              :class="isRefreshing ? 'animate-spin text-amber-600' : ''"
            />
            刷新配额
          </UiButton>
        </UiTooltip>

        <UiTooltip content="前往模型管理查看账号与密钥详情">
          <UiButton
            variant="ghost"
            size="sm"
            class="text-slate-500 hover:text-slate-800 hover:bg-slate-100/60 text-xs font-medium px-2.5 py-1.5 h-auto"
            @click="emit('navigate-governance')"
          >
            管理账号
            <Icons name="chevron-right" size="13" class="ml-0.5" />
          </UiButton>
        </UiTooltip>
      </div>
    </div>

    <!-- 指标布局区：左侧账户状态，右侧现代精简指标 -->
    <div class="grid grid-cols-1 lg:grid-cols-12 gap-4 items-stretch">
      <!-- 账户可用性状态与矩阵槽位 (占 4 列) -->
      <div class="lg:col-span-4 flex min-h-0 flex-col justify-between p-3 rounded-lg bg-slate-50/60 border border-slate-100/80">
        <div class="flex shrink-0 self-start w-full items-center justify-between text-xs text-slate-500 mb-2 font-medium">
          <span class="inline-flex items-center gap-1 text-slate-700">
            <Icons name="check" size="14" class="text-slate-700" />
            账户可用性状态
          </span>
          <span
            class="px-2 py-0.5 text-[11px] font-semibold rounded-full text-white shadow-xs font-mono tracking-tight"
            :class="activeKeys.length > 0 ? 'bg-emerald-600' : 'bg-rose-600'"
            data-testid="account-ready-badge"
          >
            {{ activeKeys.length }}/{{ totalAccounts }} 账号就绪
          </span>
        </div>

        <div class="flex flex-1 flex-col justify-center">
          <div class="flex items-baseline gap-2 mb-2">
            <span class="text-3xl font-bold font-mono tracking-tight text-slate-900 tabular-nums">
              {{ availabilityRate }}%
            </span>
            <div class="flex items-center gap-1.5 ml-1 text-xs">
              <span v-if="accountTierSummary.proCount > 0" class="px-2 py-0.5 rounded-full bg-amber-50 text-amber-700 font-medium">
                {{ accountTierSummary.proCount }} Pro
              </span>
              <span v-if="accountTierSummary.standardCount > 0" class="px-2 py-0.5 rounded-full bg-sky-50 text-sky-700 font-medium">
                {{ accountTierSummary.standardCount }} 标准
              </span>
              <span v-if="accountTierSummary.calibratingCount > 0" class="px-2 py-0.5 rounded-full bg-slate-100 text-slate-500 font-medium">
                {{ accountTierSummary.calibratingCount }} 待测
              </span>
            </div>
          </div>

          <!-- 绝对严格等距矩阵：每 5 行为一组垂直排列，新增一组，外层留足 p-1 与 overflow-visible 避免边缘放大切割 -->
          <div ref="gridContainerRef" class="my-2.5 w-full overflow-x-auto overflow-y-visible p-1 space-y-2">
            <div
              v-for="(group, gIdx) in matrixGroups"
              :key="`matrix-group-${gIdx}`"
              class="grid grid-rows-5 grid-flow-col gap-[3px] w-max p-0.5"
              data-testid="slot-heatmap-grid"
            >
              <!-- 已分配账号槽位 (无边框，纯方块) -->
              <UiTooltip
                v-for="item in group.items"
                :key="item.key.id"
                :content="`${item.tooltipText} · 点击查看单账号测定画像`"
              >
                <div
                  data-testid="slot-heatmap-cell"
                  role="button"
                  tabindex="0"
                  :aria-label="`查看账号 ${item.key.id}（${levelAriaLabel(item.level)}）测定画像`"
                  class="w-3.5 h-3.5 rounded-[2px] transition-transform duration-150 hover:scale-125 hover:z-20 cursor-pointer shrink-0 focus:outline-hidden focus:ring-2 focus:ring-amber-500"
                  :class="[
                    item.heatClass,
                    selectedKeyId === item.key.id ? 'ring-2 ring-amber-500 scale-110 z-30' : ''
                  ]"
                  @click="selectKeyForDetail(item.key.id)"
                  @keydown.enter.prevent="selectKeyForDetail(item.key.id)"
                  @keydown.space.prevent="selectKeyForDetail(item.key.id)"
                />
              </UiTooltip>

              <!-- 未占用预留槽位 (严格等距，沉稳灰阶实体方块) -->
              <UiTooltip
                v-for="i in group.emptyCount"
                :key="`empty-slot-${gIdx}-${i}`"
                content="未配置槽位 · 接入新账号后将自动点亮"
              >
                <div
                  data-testid="slot-heatmap-empty"
                  class="w-3.5 h-3.5 rounded-[2px] bg-[#d0d7de] transition-colors hover:bg-[#afb8c1] shrink-0"
                />
              </UiTooltip>
            </div>
          </div>
        </div>

        <!-- 冷却恢复提示 -->
        <div class="mt-3 pt-2.5 border-t border-slate-200/50 text-xs">
          <div v-if="coolingKeys.length > 0 && nextRecovery" class="flex items-center gap-1.5 text-rose-600">
            <Icons name="warning" size="13" class="shrink-0" />
            <span class="truncate">
              最近解冻: <strong class="font-mono font-semibold">{{ nextRecovery.label }}</strong>
              <span v-if="nextRecovery.key" class="text-slate-400 font-mono text-[11px] ml-1">({{ nextRecovery.key.id }})</span>
            </span>
          </div>
          <div v-else class="flex items-center gap-1.5 text-slate-500">
            <Icons name="check" size="13" class="text-emerald-600 shrink-0" />
            <span>无冷却账号 · 全量槽位就绪</span>
          </div>
        </div>
      </div>

      <!-- 右侧现代精简排版：标题置顶，上方剩余空间居中放数字，底部对齐周期测定与容量水位 (占 8 列) -->
      <div class="lg:col-span-8 flex flex-col justify-between">
        <!-- 上方：周期基准用量（标题置顶，数字在剩余空间居中，左右留出边距） -->
        <div class="flex-1 flex flex-col px-3 sm:px-4 pt-1">
          <div class="flex items-center justify-between gap-1 text-xs text-slate-500 font-medium shrink-0">
            <div class="flex items-center gap-1">
              <Icons name="activity" size="13" class="text-slate-700" />
              周期基准用量
            </div>
            <span class="text-[11px] text-slate-400 font-normal">多账号真实测定的单账号周期用量基准</span>
          </div>
          <div class="flex-1 flex flex-col justify-center py-2">
            <div class="grid grid-cols-3 gap-3">
              <div class="flex flex-col">
                <span class="text-[10px] text-slate-400 mb-1.5">5小时用量</span>
                <span class="text-4xl sm:text-5xl font-bold font-mono tracking-tight text-slate-900 tabular-nums leading-none">
                  {{ factualCycleSummary.avg5h > 0 ? formatTokenHuman(factualCycleSummary.avg5h) : '--' }}
                </span>
              </div>
              <div class="flex flex-col">
                <span class="text-[10px] text-slate-400 mb-1.5">自然周用量</span>
                <span class="text-4xl sm:text-5xl font-bold font-mono tracking-tight text-slate-900 tabular-nums leading-none">
                  {{ factualCycleSummary.avgWeekly > 0 ? formatTokenHuman(factualCycleSummary.avgWeekly) : '--' }}
                </span>
              </div>
              <div class="flex flex-col">
                <span class="text-[10px] text-slate-400 mb-1.5">自然月用量</span>
                <span class="text-4xl sm:text-5xl font-bold font-mono tracking-tight text-slate-900 tabular-nums leading-none">
                  {{ factualCycleSummary.avgMonthly > 0 ? formatTokenHuman(factualCycleSummary.avgMonthly) : '--' }}
                </span>
              </div>
            </div>
          </div>
        </div>

        <!-- 底部区：容量水位 + 周期统计3个小面板紧贴底部 -->
        <div class="flex flex-col gap-[30px] mt-auto">
          <!-- Gemini 容量水位条 (横向双列紧凑展开) -->
          <div class="p-3 rounded-lg bg-slate-50/60 border border-slate-100/80">
            <div class="flex items-center justify-between text-xs text-slate-500 mb-1.5 font-medium">
              <span class="inline-flex items-center gap-1 text-slate-700">
                <Icons name="sparkles" size="14" class="text-slate-700" />
                Gemini 容量水位
              </span>
              <span class="text-[11px] text-slate-400">基于当前 {{ activeKeys.length }} 个就绪账号剩余额度</span>
            </div>
            <div class="grid grid-cols-1 sm:grid-cols-2 gap-9">
              <!-- 5 小时窗口水位 -->
              <div class="space-y-1">
                <div class="flex items-center justify-between text-xs">
                  <span class="font-medium text-slate-700">5小时窗口</span>
                  <span class="font-mono font-semibold tabular-nums text-right shrink-0" data-testid="gemini-h5-percent" :class="getProgressColor(aggregatedQuotas.gemini.h5Percent).text">
                    {{ formatWaterPercent(aggregatedQuotas.gemini.h5Percent) }}
                  </span>
                </div>
                <div class="w-full bg-slate-200/80 rounded-full h-1.5 overflow-hidden">
                  <div
                    class="h-full rounded-full transition-all duration-300"
                    :class="getProgressColor(aggregatedQuotas.gemini.h5Percent).bar"
                    :style="{ width: waterBarWidth(aggregatedQuotas.gemini.h5Percent) }"
                  />
                </div>
                <div class="text-[10px] text-slate-400 text-right truncate">
                  {{ aggregatedQuotas.gemini.h5Hint }}
                </div>
              </div>
              <!-- 周度窗口水位 -->
              <div class="space-y-1">
                <div class="flex items-center justify-between text-xs">
                  <span class="font-medium text-slate-700">周度窗口</span>
                  <span class="font-mono font-semibold tabular-nums text-right shrink-0" data-testid="gemini-weekly-percent" :class="getProgressColor(aggregatedQuotas.gemini.weeklyPercent).text">
                    {{ formatWaterPercent(aggregatedQuotas.gemini.weeklyPercent) }}
                  </span>
                </div>
                <div class="w-full bg-slate-200/80 rounded-full h-1.5 overflow-hidden">
                  <div
                    class="h-full rounded-full transition-all duration-300"
                    :class="getProgressColor(aggregatedQuotas.gemini.weeklyPercent).bar"
                    :style="{ width: waterBarWidth(aggregatedQuotas.gemini.weeklyPercent) }"
                  />
                </div>
                <div class="text-[10px] text-slate-400 text-right truncate">
                  {{ aggregatedQuotas.gemini.weeklyHint }}
                </div>
              </div>
            </div>
          </div>

          <!-- 5小时 / 自然周 / 自然月 测定基准 (输入大字体 + 右侧紧凑三要素，底部对齐) -->
          <div class="grid grid-cols-1 sm:grid-cols-3 gap-2">
          <!-- 5小时 -->
          <div class="p-2.5 rounded-lg bg-slate-50/60 border border-slate-100/80 flex flex-col justify-between">
            <div class="text-xs font-semibold text-slate-700 mb-1">5小时</div>
            <div class="flex items-center justify-between gap-2">
              <div class="flex flex-col min-w-0">
                <span class="text-[10px] text-slate-400">输入 Token</span>
                <span class="text-xl font-bold font-mono tracking-tight text-slate-800 tabular-nums">
                  {{ factualCycleSummary.avgPrompt5h > 0 ? formatTokenHuman(factualCycleSummary.avgPrompt5h) : (factualCycleSummary.avg5h > 0 ? formatTokenHuman(factualCycleSummary.avg5h) : '--') }}
                </span>
              </div>
              <div class="flex flex-col text-[11px] font-mono text-slate-600 space-y-0.5 shrink-0 text-right">
                <div class="flex items-center justify-end gap-1" title="平均单账号 5h 输出 Completion Token">
                  <Icons name="arrow-up-right" size="11" class="text-amber-600" />
                  <span>{{ factualCycleSummary.avgComp5h > 0 ? formatTokenHuman(factualCycleSummary.avgComp5h) : '--' }}</span>
                </div>
                <div class="flex items-center justify-end gap-1" title="平均单账号 5h 缓存命中 Token">
                  <Icons name="database" size="11" class="text-sky-600" />
                  <span>{{ factualCycleSummary.avgCached5h > 0 ? formatTokenHuman(factualCycleSummary.avgCached5h) : '--' }}</span>
                </div>
                <div class="flex items-center justify-end gap-1" title="平均单账号 5h 承载调用次数">
                  <Icons name="repeat" size="11" class="text-purple-600" />
                  <span>{{ factualCycleSummary.avgRequests5h > 0 ? `${factualCycleSummary.avgRequests5h}次` : '--' }}</span>
                </div>
              </div>
            </div>
          </div>

          <!-- 自然周 -->
          <div class="p-2.5 rounded-lg bg-slate-50/60 border border-slate-100/80 flex flex-col justify-between">
            <div class="text-xs font-semibold text-slate-700 mb-1">自然周</div>
            <div class="flex items-center justify-between gap-2">
              <div class="flex flex-col min-w-0">
                <span class="text-[10px] text-slate-400">输入 Token</span>
                <span class="text-xl font-bold font-mono tracking-tight text-slate-800 tabular-nums">
                  {{ factualCycleSummary.avgPromptWeekly > 0 ? formatTokenHuman(factualCycleSummary.avgPromptWeekly) : (factualCycleSummary.avgWeekly > 0 ? formatTokenHuman(factualCycleSummary.avgWeekly) : '--') }}
                </span>
              </div>
              <div class="flex flex-col text-[11px] font-mono text-slate-600 space-y-0.5 shrink-0 text-right">
                <div class="flex items-center justify-end gap-1" title="单账号周度输出 Token">
                  <Icons name="arrow-up-right" size="11" class="text-amber-600" />
                  <span>{{ factualCycleSummary.avgCompWeekly > 0 ? formatTokenHuman(factualCycleSummary.avgCompWeekly) : '--' }}</span>
                </div>
                <div class="flex items-center justify-end gap-1" title="单账号周度缓存命中 Token">
                  <Icons name="database" size="11" class="text-sky-600" />
                  <span>{{ factualCycleSummary.avgCachedWeekly > 0 ? formatTokenHuman(factualCycleSummary.avgCachedWeekly) : '--' }}</span>
                </div>
                <div class="flex items-center justify-end gap-1" title="单账号周度累计调用次数">
                  <Icons name="repeat" size="11" class="text-purple-600" />
                  <span>{{ factualCycleSummary.avgRequestsWeekly > 0 ? `${factualCycleSummary.avgRequestsWeekly}次` : '--' }}</span>
                </div>
              </div>
            </div>
          </div>

          <!-- 自然月 -->
          <div class="p-2.5 rounded-lg bg-slate-50/60 border border-slate-100/80 flex flex-col justify-between">
            <div class="text-xs font-semibold text-slate-700 mb-1">自然月</div>
            <div class="flex items-center justify-between gap-2">
              <div class="flex flex-col min-w-0">
                <span class="text-[10px] text-slate-400">输入 Token</span>
                <span class="text-xl font-bold font-mono tracking-tight text-slate-800 tabular-nums">
                  {{ factualCycleSummary.avgPromptMonthly > 0 ? formatTokenHuman(factualCycleSummary.avgPromptMonthly) : (factualCycleSummary.avgMonthly > 0 ? formatTokenHuman(factualCycleSummary.avgMonthly) : '--') }}
                </span>
              </div>
              <div class="flex flex-col text-[11px] font-mono text-slate-600 space-y-0.5 shrink-0 text-right">
                <div class="flex items-center justify-end gap-1" title="单账号月度输出 Token">
                  <Icons name="arrow-up-right" size="11" class="text-amber-600" />
                  <span>{{ factualCycleSummary.avgCompMonthly > 0 ? formatTokenHuman(factualCycleSummary.avgCompMonthly) : '--' }}</span>
                </div>
                <div class="flex items-center justify-end gap-1" title="单账号月度缓存命中 Token">
                  <Icons name="database" size="11" class="text-sky-600" />
                  <span>{{ factualCycleSummary.avgCachedMonthly > 0 ? formatTokenHuman(factualCycleSummary.avgCachedMonthly) : '--' }}</span>
                </div>
                <div class="flex items-center justify-end gap-1" title="单账号月度累计调用次数">
                  <Icons name="repeat" size="11" class="text-purple-600" />
                  <span>{{ factualCycleSummary.avgRequestsMonthly > 0 ? `${factualCycleSummary.avgRequestsMonthly}次` : '--' }}</span>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>

    <!-- 单账号专属四要素精确画像抽屉/展开区 (Swiss Minimalist Refined) -->
    <div
      v-if="selectedKeyData"
      data-testid="single-account-detail-card"
      class="mt-4 p-4 rounded-xl bg-white/45 backdrop-blur-xs text-slate-800 shadow-xs border border-white/50 transition-all duration-300 animate-in fade-in"
    >
      <div class="flex flex-wrap items-center justify-between gap-3 pb-3 mb-3 border-b border-slate-200/50">
        <div class="flex flex-wrap items-center gap-2">
          <span class="w-2 h-2 rounded-full bg-amber-500 animate-pulse" />
          <h3 class="text-sm font-semibold tracking-tight text-slate-900">
            单账号周期额度画像 · <span class="font-mono text-slate-700">{{ selectedKeyData.key.id }}</span>
          </h3>
          <span
            class="px-2 py-0.5 text-[10px] font-medium rounded-full uppercase"
            :class="{
              'bg-emerald-50 text-emerald-700 border border-emerald-200/60': selectedKeyData.usage?.calibration_status === 'benchmarked',
              'bg-sky-50 text-sky-700 border border-sky-200/60': selectedKeyData.usage?.calibration_status === 'estimated',
              'bg-slate-100/80 text-slate-600 border border-slate-200/60': !selectedKeyData.usage?.calibration_status || selectedKeyData.usage?.calibration_status === 'calibrating',
            }"
          >
            {{ selectedKeyData.usage?.calibration_status === 'benchmarked' ? '已实测验证 (BENCHMARKED)' : (selectedKeyData.usage?.calibration_status === 'estimated' ? '斜率推算 (ESTIMATED)' : '动态校准中 (CALIBRATING)') }}
          </span>
          <span class="text-xs text-slate-500">
            等级: <strong class="text-slate-700 uppercase font-mono">{{ selectedKeyData.usage?.account_tier || 'UNKNOWN' }}</strong>
            <span class="text-slate-400 font-mono ml-1">(置信度 {{ Math.round((selectedKeyData.usage?.confidence || 0) * 100) }}%)</span>
          </span>
        </div>
        <button
          class="text-xs text-slate-500 hover:text-slate-800 px-2.5 py-1 rounded-md bg-white/70 border border-white/60 shadow-2xs hover:bg-white transition-colors cursor-pointer"
          @click="selectedKeyId = null"
        >
          关闭画像 ✕
        </button>
      </div>

      <!-- 最近一次探测异常提示 -->
      <div
        v-if="selectedKeyData.testRes && selectedKeyData.testRes.success === false"
        class="mb-3 px-3 py-2 rounded-lg bg-rose-50/80 border border-rose-200/70 text-xs text-rose-700 flex items-center justify-between"
      >
        <span>
          <strong>探测异常：</strong>
          {{ selectedKeyData.testRes.message || '上游连接或配额读取失败' }}
        </span>
        <span class="font-mono text-[11px] text-rose-500">
          {{ selectedKeyData.testRes.error_code || 'PROBE_FAILED' }}
        </span>
      </div>

      <!-- 四要素结构指标：Prompt / Completion / Cached / Requests -->
      <div class="grid grid-cols-2 sm:grid-cols-4 gap-3 mb-3">
        <!-- 输入 Tokens -->
        <div class="p-3 rounded-lg bg-white/70 shadow-2xs">
          <div class="text-[11px] text-slate-500 mb-0.5 inline-flex items-center gap-1">
            <Icons name="arrow-down-left" size="12" class="text-slate-400" />
            输入 (Prompt)
          </div>
          <div class="text-lg font-bold font-mono tracking-tight text-slate-800 tabular-nums">
            {{ formatTokenHuman(selectedKeyData.usage?.completed_5h_stats?.prompt_tokens ?? selectedKeyData.usage?.window_5h?.prompt_tokens ?? 0) }}
          </div>
          <div class="text-[10px] text-slate-400 mt-0.5 font-mono tabular-nums">
            5h 累计: {{ (selectedKeyData.usage?.window_5h?.prompt_tokens ?? 0).toLocaleString() }}
          </div>
        </div>

        <!-- 输出 Tokens -->
        <div class="p-3 rounded-lg bg-white/70 shadow-2xs">
          <div class="text-[11px] text-slate-500 mb-0.5 inline-flex items-center gap-1">
            <Icons name="arrow-up-right" size="12" class="text-slate-400" />
            输出 (Completion)
          </div>
          <div class="text-lg font-bold font-mono tracking-tight text-slate-800 tabular-nums">
            {{ formatTokenHuman(selectedKeyData.usage?.completed_5h_stats?.completion_tokens ?? selectedKeyData.usage?.window_5h?.completion_tokens ?? 0) }}
          </div>
          <div class="text-[10px] text-slate-400 mt-0.5 font-mono tabular-nums">
            权重 3x: {{ ((selectedKeyData.usage?.window_5h?.completion_tokens ?? 0) * 3).toLocaleString() }} eq
          </div>
        </div>

        <!-- 缓存命中 Tokens -->
        <div class="p-3 rounded-lg bg-white/70 shadow-2xs">
          <div class="text-[11px] text-slate-500 mb-0.5 inline-flex items-center gap-1">
            <Icons name="database" size="12" class="text-slate-400" />
            缓存命中 (Cached)
          </div>
          <div class="text-lg font-bold font-mono tracking-tight text-slate-800 tabular-nums">
            {{ formatTokenHuman(selectedKeyData.usage?.completed_5h_stats?.cached_tokens ?? selectedKeyData.usage?.window_5h?.cached_tokens ?? 0) }}
          </div>
          <div class="text-[10px] text-slate-400 mt-0.5">
            命中节省配额
          </div>
        </div>

        <!-- 调用次数 (Requests) -->
        <div class="p-3 rounded-lg bg-white/70 shadow-2xs">
          <div class="text-[11px] text-slate-500 mb-0.5 inline-flex items-center gap-1">
            <Icons name="repeat" size="12" class="text-slate-400" />
            累计调用次数 (Requests)
          </div>
          <div class="text-lg font-bold font-mono tracking-tight text-slate-800 tabular-nums">
            {{ (selectedKeyData.usage?.completed_5h_stats?.requests ?? selectedKeyData.usage?.window_5h?.requests ?? 0) }} <span class="text-xs font-normal text-slate-500 font-sans">次</span>
          </div>
          <div class="text-[10px] text-slate-400 mt-0.5 font-mono tabular-nums">
            均次消耗: {{ selectedKeyData.usage?.window_5h?.requests ? Math.round((selectedKeyData.usage.window_5h.total_tokens || 0) / selectedKeyData.usage.window_5h.requests).toLocaleString() : '--' }} tk/req
          </div>
        </div>
      </div>

      <!-- 周度打满周期累计（持久化完整周期实测） -->
      <div
        v-if="selectedKeyData.usage?.completed_weekly_stats && selectedKeyData.usage.completed_weekly_stats.count > 0"
        class="mb-3 px-3 py-2 rounded-lg bg-white/60 border border-slate-100 text-xs text-slate-700 flex flex-wrap items-center justify-between gap-2"
        data-testid="weekly-cycle-summary"
      >
        <span class="font-medium text-slate-800">自然周打满周期实测（持久化）</span>
        <span>
          已结算 <strong class="font-mono text-slate-900">{{ selectedKeyData.usage.completed_weekly_stats.count }}</strong> 轮 · 平均
          <strong class="font-mono text-slate-900">{{ formatTokenHuman(selectedKeyData.usage.completed_weekly_stats.avg_tokens) }}</strong>
          <span class="text-[10px] text-slate-400 ml-1 font-mono">
            总量 {{ formatTokenHuman(selectedKeyData.usage.completed_weekly_stats.total_tokens) }}
          </span>
        </span>
      </div>

      <!-- 额度容量双轨测定结论 -->
      <div class="grid grid-cols-1 md:grid-cols-3 gap-3 text-xs bg-white/70 p-3 rounded-lg shadow-2xs">
        <div>
          <span class="text-slate-500">5h 周期测定容量:</span>
          <span class="text-slate-900 font-bold font-mono tabular-nums ml-1.5">
            {{ selectedKeyData.usage?.estimated_capacity_5h ? formatTokenHuman(selectedKeyData.usage.estimated_capacity_5h) : '--' }}
          </span>
          <span class="text-[10px] text-slate-400 font-mono block mt-0.5">
            剩余: {{ selectedKeyData.usage?.estimated_tokens_remaining_5h ? formatTokenHuman(selectedKeyData.usage.estimated_tokens_remaining_5h) : '--' }}
            ({{ selectedKeyData.quota.gemini.h5Fraction == null ? '--' : `${Math.round(selectedKeyData.quota.gemini.h5Fraction * 100)}%` }})
          </span>
        </div>
        <div>
          <span class="text-slate-500">自然周理论容量:</span>
          <span class="text-slate-900 font-bold font-mono tabular-nums ml-1.5">
            {{ selectedKeyData.usage?.estimated_capacity_weekly ? formatTokenHuman(selectedKeyData.usage.estimated_capacity_weekly) : '--' }}
          </span>
          <span class="text-[10px] text-slate-400 font-mono block mt-0.5">
            周度余量水位: {{ selectedKeyData.quota.gemini.weeklyFraction == null ? '--' : `${Math.round(selectedKeyData.quota.gemini.weeklyFraction * 100)}%` }}
          </span>
        </div>
        <div>
          <span class="text-slate-500">月度等效承载量:</span>
          <span class="text-slate-900 font-bold font-mono tabular-nums ml-1.5">
            {{ selectedKeyData.usage?.estimated_capacity_weekly ? formatTokenHuman(Math.round(selectedKeyData.usage.estimated_capacity_weekly * 4.33)) : '--' }}
          </span>
          <span class="text-[10px] text-slate-400 font-mono block mt-0.5">
            近30天实跑: {{ formatTokenHuman(selectedKeyData.usage?.window_monthly?.total_tokens ?? 0) }}
          </span>
        </div>
      </div>
    </div>
  </div>
</template>
