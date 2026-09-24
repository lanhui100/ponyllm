<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from 'vue';
import type { KeyView, KeyTestView } from '../types/admin';
import Icons from './ui/Icons.vue';
import UiTooltip from './ui/UiTooltip.vue';
import UiButton from './ui/UiButton.vue';

const props = withDefaults(
  defineProps<{
    keys: KeyView[];
    keyTestResults?: Record<string, KeyTestView>;
    isRefreshing?: boolean;
    adminWriteEnabled?: boolean;
  }>(),
  {
    keyTestResults: () => ({}),
    isRefreshing: false,
    adminWriteEnabled: true,
  }
);

const emit = defineEmits<{
  (e: 'refresh-quotas'): void;
  (e: 'cooldown-expired'): void;
  (e: 'navigate-governance'): void;
  (e: 'select-key', key: KeyView): void;
}>();

const selectedKeyForDetails = ref<KeyView | null>(null);

function openKeyDetails(key: KeyView) {
  selectedKeyForDetails.value = key;
  emit('select-key', key);
}

function handleKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape' && selectedKeyForDetails.value) {
    selectedKeyForDetails.value = null;
  }
}

watch(selectedKeyForDetails, (val) => {
  if (val) {
    window.addEventListener('keydown', handleKeydown);
  } else {
    window.removeEventListener('keydown', handleKeydown);
  }
});

onUnmounted(() => {
  window.removeEventListener('keydown', handleKeydown);
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
const recordedSnapshots = new Map<string, { remaining: number; fetchedAt: number }>();

watch(
  () => props.keys,
  (newKeys) => {
    const now = Date.now();
    for (const k of newKeys) {
      if (k.state === 'cooling_down' && k.cooldown_remaining_secs != null) {
        recordedSnapshots.set(k.id, {
          remaining: k.cooldown_remaining_secs,
          fetchedAt: now,
        });
      } else {
        recordedSnapshots.delete(k.id);
      }
    }
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
  const currentNow = nowMs.value;
  const snapshot = recordedSnapshots.get(k.id);
  if (snapshot) {
    const elapsedSecs = Math.floor((currentNow - snapshot.fetchedAt) / 1000);
    return Math.max(0, snapshot.remaining - elapsedSecs);
  }
  if (k.cooldown_remaining_secs != null) {
    return Math.max(0, k.cooldown_remaining_secs);
  }
  if (k.cooldown_reset_at) {
    const resetMs = new Date(k.cooldown_reset_at).getTime();
    if (!Number.isNaN(resetMs)) {
      return Math.max(0, Math.floor((resetMs - currentNow) / 1000));
    }
  }
  return null;
}

function formatCooldownDuration(secs: number | null): string {
  if (secs == null || secs <= 0) return '';
  const days = Math.floor(secs / 86400);
  const hours = Math.floor((secs % 86400) / 3600);
  const minutes = Math.floor((secs % 3600) / 60);
  if (days > 0) return `${days}天${hours}小时`;
  if (hours > 0) return `${hours}小时${minutes}分`;
  if (minutes > 0) return `${minutes}分`;
  return `${secs}秒`;
}

// 统计信息
const totalAccounts = computed(() => props.keys.length);

function isKeyCoolingDown(k: KeyView): boolean {
  if (k.state === 'cooling_down') return true;
  const testResult = props.keyTestResults[k.id];
  if (!testResult) return false;
  // 必须具有真实 quota_groups 或 quota 配额探测数据，才参与根据周限流判定冷却
  if (!testResult.quota_groups && !testResult.quota) return false;
  // 仅 Gemini 系列参与冷却判定：上游已不再下发 Claude 额度
  const q = extractKeyQuota(k, testResult);
  return q.gemini.weeklyFraction <= 0;
}

const coolingKeys = computed(() => {
  return props.keys.filter((k) => isKeyCoolingDown(k));
});

const activeKeys = computed(() => {
  return props.keys.filter((k) => !isKeyCoolingDown(k));
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
      if (testResult) {
        const q = extractKeyQuota(k, testResult);
        if (q.gemini.weeklyFraction <= 0 && q.gemini.weeklyResetHint && q.gemini.weeklyResetHint !== '已就绪' && q.gemini.weeklyResetHint !== '冷却保护中') {
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
  h5Fraction: number;
  h5ResetHint: string;
  weeklyFraction: number;
  weeklyResetHint: string;
}

// 仅查询显示 Gemini 系列额度：上游已不再下发 Claude 额度，Claude/GPT/3P
// 分组与 claude/gpt/sonnet/opus 模型直接跳过（不归入 Gemini，防止残留污染水位）。
function extractKeyQuota(k: KeyView, testResult?: KeyTestView): { gemini: ExtractedQuota } {
  const isCooling = k.state === 'cooling_down';
  const defaultCooling: ExtractedQuota = {
    h5Fraction: 0,
    h5ResetHint: '冷却保护中',
    weeklyFraction: 0,
    weeklyResetHint: '冷却保护中',
  };
  const defaultPending: ExtractedQuota = {
    h5Fraction: 0,
    h5ResetHint: '等待刷新',
    weeklyFraction: 0,
    weeklyResetHint: '等待刷新',
  };

  if (!testResult) {
    if (isCooling) {
      return { gemini: { ...defaultCooling } };
    }
    return { gemini: { ...defaultPending } };
  }

  const res = {
    gemini: { h5Fraction: 0, h5ResetHint: '已就绪', weeklyFraction: 0, weeklyResetHint: '已就绪' },
  };

  // 1. Quota groups
  if (testResult.quota_groups && testResult.quota_groups.length > 0) {
    // 记录 Gemini 是否真实出现周窗口：摘要缺失 ≠ 耗尽（retrieveUserQuotaSummary
    // 只有 4s 超时，冷建连下整组缺席时不做有罪推定；未知周默认健康，与模型管理页 ?? 1.0 对齐）
    let geminiWeeklySeen = false;
    for (const group of testResult.quota_groups) {
      const name = (group.display_name || '').toLowerCase();
      const isClaudeGroup = name.includes('claude') || name.includes('gpt') || name.includes('3p');
      if (isClaudeGroup) continue;
      const target = res.gemini;

      for (const b of group.buckets || []) {
        const win = (b.window || '').toLowerCase();
        const bId = (b.bucket_id || '').toLowerCase();
        // 周识别与模型管理页 extractCompactQuotas 同口径（含 description/display_name 的
        // week/周/7d 变体），避免 fail-open 后把“仅在描述中标注的周桶”误写入 5h 并漏报冷却
        const bDesc = (b.description || '').toLowerCase();
        const bDisp = (b.display_name || '').toLowerCase();
        const isWeekly = win === 'weekly' || bId.includes('week') || bDesc.includes('week') || bDisp.includes('周') || bId.includes('7d');
        const fraction = b.remaining_fraction ?? 0;
        const resetHint = b.time_until_reset || b.reset_time_beijing || '已就绪';

        if (isWeekly) {
          target.weeklyFraction = fraction;
          target.weeklyResetHint = resetHint;
          geminiWeeklySeen = true;
        } else {
          target.h5Fraction = fraction;
          target.h5ResetHint = resetHint;
        }
      }
    }
    if (!geminiWeeklySeen) {
      res.gemini.weeklyFraction = 1.0;
      res.gemini.weeklyResetHint = '已就绪';
    }
  } else if (testResult.quota && testResult.quota.length > 0) {
    // 平铺 fetchAvailableModels 只表达 5h 滚动余量（33 个模型均无周窗口标识）：
    // 按同系列最小值聚合为 5h 水位（与模型管理页 extractCompactQuotas 取最小值一致，
    // 替代此前的遍历覆盖/末值胜出）；周水位无信号时记健康，冷却判定交还后端 state。
    let geminiH5: { fraction: number; hint: string } | null = null;
    let geminiWeeklySeen = false;
    for (const q of testResult.quota) {
      const mId = q.model_id.toLowerCase();
      const isClaude = mId.includes('claude') || mId.includes('gpt') || mId.includes('sonnet') || mId.includes('opus');
      if (isClaude) continue;
      const fraction = q.remaining_fraction ?? 0;
      const resetHint = q.time_until_reset || q.reset_time_beijing || '已就绪';
      const isWeekly = mId.includes('week') || mId.includes('7d') || (q.time_until_reset && (q.time_until_reset.includes('天') || q.time_until_reset.includes('d')));

      if (isWeekly) {
        geminiWeeklySeen = true;
        res.gemini.weeklyFraction = fraction;
        res.gemini.weeklyResetHint = resetHint;
      } else {
        if (geminiH5 === null || fraction < geminiH5.fraction) {
          geminiH5 = { fraction, hint: resetHint };
        }
      }
    }
    if (geminiH5 !== null) {
      res.gemini.h5Fraction = geminiH5.fraction;
      res.gemini.h5ResetHint = geminiH5.hint;
    }
    if (!geminiWeeklySeen) {
      res.gemini.weeklyFraction = 1.0;
      res.gemini.weeklyResetHint = '已就绪';
    }
  } else {
    // 探测结果不存在 quota / quota_groups 时（如测试失败或无配额信息）
    res.gemini.h5Fraction = 0;
    res.gemini.weeklyFraction = 0;
    res.gemini.h5ResetHint = isCooling ? '冷却保护中' : '等待刷新';
    res.gemini.weeklyResetHint = isCooling ? '冷却保护中' : '等待刷新';
  }

  return res;
}

// 聚合有效可用账户在 5h 和周度窗口的水位百分比
const aggregatedQuotas = computed(() => {
  const active = activeKeys.value;
  if (active.length === 0) {
    return {
      gemini: { h5Percent: 0, h5Hint: '所有账号冷却中', weeklyPercent: 0, weeklyHint: '所有账号冷却中' },
    };
  }

  let geminiH5Sum = 0;
  let geminiWeeklySum = 0;
  let geminiH5Hint = '';
  let geminiWeeklyHint = '';

  for (const k of active) {
    const q = extractKeyQuota(k, props.keyTestResults[k.id]);
    geminiH5Sum += q.gemini.h5Fraction;
    geminiWeeklySum += q.gemini.weeklyFraction;

    if (!geminiH5Hint && q.gemini.h5ResetHint !== '已就绪') geminiH5Hint = q.gemini.h5ResetHint;
    if (!geminiWeeklyHint && q.gemini.weeklyResetHint !== '已就绪') geminiWeeklyHint = q.gemini.weeklyResetHint;
  }

  const count = active.length;
  return {
    gemini: {
      h5Percent: Math.round((geminiH5Sum / count) * 100),
      h5Hint: geminiH5Hint || '配额充足',
      weeklyPercent: Math.round((geminiWeeklySum / count) * 100),
      weeklyHint: geminiWeeklyHint || '配额充足',
    },
  };
});

// 针对各账号提取健康评分与热力方块状态
// 纯粹绿色单色系色阶（GitHub Pure Green Monochrome Scale）：
// 绝无杂色（无红、无黄、无额外背景底板）。
// 从完全未激活的沉静冷浅灰 (#ebedf0)，到冷却状态的极淡微绿，
// 再随可用额度由浅入深逐阶跃迁至充沛深翠绿 (#9be9a8 -> #40c463 -> #30a14e -> #216e39)
export type SlotHeatLevel = 'cooling' | 'low' | 'medium' | 'high' | 'full';

export interface HeatSlotItem {
  key: KeyView;
  level: SlotHeatLevel;
  heatClass: string;
  tooltipText: string;
  isCooling: boolean;
}

const slotMatrix = computed<HeatSlotItem[]>(() => {
  return props.keys.map((k) => {
    const testResult = props.keyTestResults[k.id];
    const quota = extractKeyQuota(k, testResult);
    // 若后端标记为冷却，或当前探测结果中 Gemini 周限流已耗尽归零，均视为冷却保护中
    const isWeeklyZero = quota.gemini.weeklyFraction <= 0;
    const isCooling = k.state === 'cooling_down' || isWeeklyZero;

    let email = k.id;
    if (email.startsWith('ag-')) {
      email = email.slice(3);
    }

    const usage = testResult?.usage || k.usage;
    let tierBadge = '';
    let usageSummary = '';
    if (usage) {
      const tierLabel = usage.account_tier === 'pro' ? '⭐ Pro 会员' : usage.account_tier === 'standard' ? '🔹 标准会员' : usage.account_tier === 'calibrating' ? '🔄 校准中' : usage.account_tier === 'unknown' ? '⏳ 待调用' : '⚪ 普通/免费';
      tierBadge = ` [${tierLabel}]`;
      const h5Tokens = (usage.window_5h.total_tokens / 1000).toFixed(1);
      const capTokens = usage.estimated_capacity_5h ? ` / 测算总额度 ~${(usage.estimated_capacity_5h / 1000).toFixed(0)}k` : '';
      const wTokens = (usage.window_weekly.total_tokens / 1000).toFixed(1);
      usageSummary = `\n5h用量: ${h5Tokens}k tokens (${usage.window_5h.requests}次)${capTokens}\n周用量: ${wTokens}k tokens (${usage.window_weekly.requests}次)`;
    }

    if (isCooling) {
      const remaining = cooldownRemainingSecs(k);
      const label = formatCooldownDuration(remaining);
      const hint = label
        ? `预计 ${label} 后解冻`
        : (quota.gemini.weeklyResetHint !== '冷却保护中' && quota.gemini.weeklyResetHint !== '已就绪'
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

    if (!testResult) {
      return {
        key: k,
        level: 'low',
        // 尚未探测或未持久化状态：采用低饱和沉静浅灰绿，提示等待刷新
        heatClass: 'bg-[#d0d7de]',
        tooltipText: `账号: ${email}${tierBadge}\n状态: 等待刷新配额${usageSummary}`,
        isCooling: false,
      };
    }

    // 以 Gemini 配额为核心判断等级（兼顾 5h 即时爆发余量与周度余量）
    const g5hFraction = quota.gemini.h5Fraction;
    const g5h = Math.round(g5hFraction * 100);
    const gWeekly = Math.round(quota.gemini.weeklyFraction * 100);

    const baseTooltip = `账号: ${email}${tierBadge}\nGemini: 5h余量 ${g5h}% · 周余量 ${gWeekly}%${usageSummary}`;

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
  // 列数 = Math.floor((width + 3) / 17)
  const cols = Math.floor((containerWidth.value + 3) / 17);
  return Math.max(10, cols);
});

const totalSlots = computed(() => totalColumns.value * 5);

const emptySlotCount = computed(() => {
  const current = props.keys.length;
  return Math.max(0, totalSlots.value - current);
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
});

function getProgressColor(percent: number): { bar: string; text: string; bg: string } {
  if (percent > 30) {
    return { bar: 'bg-emerald-500', text: 'text-emerald-700', bg: 'bg-emerald-50' };
  } else if (percent > 10) {
    return { bar: 'bg-amber-500', text: 'text-amber-700', bg: 'bg-amber-50' };
  } else {
    return { bar: 'bg-rose-500', text: 'text-rose-700', bg: 'bg-rose-50' };
  }
}
</script>

<template>
  <div
    v-if="totalAccounts > 0"
    class="swiss-card p-5 bg-white/45 backdrop-blur-xs transition-all duration-200 border border-white/40 shadow-xs mb-6"
    :class="{ 'border-rose-200/80 bg-rose-50/20': activeKeys.length === 0 }"
  >
    <!-- 卡片头部 -->
    <div class="flex flex-wrap items-center justify-between gap-3 pb-4 mb-4 border-b border-slate-200/60">
      <div class="flex items-center gap-2.5">
        <div class="flex items-center justify-center w-7 h-7 rounded-lg bg-amber-500/10 text-amber-600">
          <Icons name="zap" size="16" />
        </div>
        <div>
          <h2 class="text-base font-bold text-slate-800 tracking-tight">Antigravity 算力池</h2>
          <p class="text-xs text-slate-500 mt-0.5">多账号配额水位聚合、5小时/周度容量监测与冷却解冻预测</p>
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

    <!-- 双栏指标网格 -->
    <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
      <!-- 栏 1: 账号可用性与恢复倒计时 -->
      <div class="flex flex-col justify-between p-3.5 rounded-lg bg-slate-50/60 border border-slate-100/80">
        <div>
          <div class="flex items-center justify-between text-xs text-slate-500 mb-2 font-medium">
            <span class="inline-flex items-center gap-1 text-slate-700">
              <Icons name="check" size="14" class="text-emerald-600" />
              账户可用性状态
            </span>
            <!-- 颜色背景反白字体徽标方案 -->
            <span
              class="px-2 py-0.5 text-[11px] font-semibold rounded-full text-white shadow-xs font-mono tracking-tight"
              :class="activeKeys.length > 0 ? 'bg-emerald-600' : 'bg-rose-600'"
              data-testid="account-ready-badge"
            >
              {{ activeKeys.length }}/{{ totalAccounts }} 账号就绪
            </span>
          </div>

          <div class="flex items-baseline gap-2 mb-2">
            <span class="text-3xl font-bold font-mono tracking-tight text-slate-900 tabular-nums">
              {{ availabilityRate }}%
            </span>
          </div>

          <!-- 绝对严格等距矩阵：移除 justify-between，横纵均由 gap-[3px] 锁定，自动排列填满整行 -->
          <div ref="gridContainerRef" class="my-2.5 w-full overflow-hidden">
            <div
              class="grid grid-rows-5 grid-flow-col gap-[3px] w-max"
              data-testid="slot-heatmap-grid"
            >
              <!-- 已分配账号槽位 (无边框，纯方块) -->
              <UiTooltip
                v-for="item in slotMatrix"
                :key="item.key.id"
                :content="item.tooltipText"
              >
                <div
                  data-testid="slot-heatmap-cell"
                  class="w-3.5 h-3.5 rounded-[2px] transition-transform duration-150 hover:scale-125 cursor-pointer shrink-0"
                  :class="item.heatClass"
                  @click="openKeyDetails(item.key)"
                />
              </UiTooltip>

              <!-- 未占用预留槽位 (严格等距，沉稳灰阶实体方块) -->
              <UiTooltip
                v-for="i in emptySlotCount"
                :key="`empty-slot-${i}`"
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

      <!-- 栏 2: Gemini 容量 (5小时即时窗口 + 周度长效续航) -->
      <div class="flex flex-col justify-between p-3.5 rounded-lg bg-slate-50/60 border border-slate-100/80">
        <div>
          <div class="flex items-center justify-between text-xs text-slate-500 mb-2.5 font-medium">
            <span class="inline-flex items-center gap-1 text-slate-700">
              <Icons name="sparkles" size="14" class="text-sky-600" />
              Gemini 容量
            </span>
            <UiTooltip content="5h 窗口由上游即时限流桶驱动，周度窗口由自然周/7天配额桶驱动，共同表征就绪账号的爆发余量与周期续航。">
              <span class="text-[11px] text-slate-400 cursor-help">即时 + 周度窗口 ⓘ</span>
            </UiTooltip>
          </div>

          <!-- 5 小时窗口水位 -->
          <div class="space-y-1.5 mb-4">
            <div class="flex items-center justify-between text-xs">
              <span class="font-medium text-slate-700">5小时窗口</span>
              <span class="font-mono font-semibold tabular-nums w-[3rem] text-right shrink-0" data-testid="gemini-h5-percent" :class="getProgressColor(aggregatedQuotas.gemini.h5Percent).text">
                {{ aggregatedQuotas.gemini.h5Percent }}%
              </span>
            </div>
            <div class="w-full bg-slate-200/80 rounded-full h-1.5 overflow-hidden">
              <div
                class="h-full rounded-full transition-all duration-300"
                :class="getProgressColor(aggregatedQuotas.gemini.h5Percent).bar"
                :style="{ width: `${aggregatedQuotas.gemini.h5Percent}%` }"
              />
            </div>
            <div class="text-[11px] text-slate-400 text-right truncate">
              {{ aggregatedQuotas.gemini.h5Hint }}
            </div>
          </div>

          <!-- 周度窗口水位 -->
          <div class="space-y-1.5">
            <div class="flex items-center justify-between text-xs">
              <span class="font-medium text-slate-700">周度窗口</span>
              <span class="font-mono font-semibold tabular-nums w-[3rem] text-right shrink-0" data-testid="gemini-weekly-percent" :class="getProgressColor(aggregatedQuotas.gemini.weeklyPercent).text">
                {{ aggregatedQuotas.gemini.weeklyPercent }}%
              </span>
            </div>
            <div class="w-full bg-slate-200/80 rounded-full h-1.5 overflow-hidden">
              <div
                class="h-full rounded-full transition-all duration-300"
                :class="getProgressColor(aggregatedQuotas.gemini.weeklyPercent).bar"
                :style="{ width: `${aggregatedQuotas.gemini.weeklyPercent}%` }"
              />
            </div>
            <div class="text-[11px] text-slate-400 text-right truncate">
              {{ aggregatedQuotas.gemini.weeklyHint }}
            </div>
          </div>
        </div>

        <div class="mt-3 pt-2.5 border-t border-slate-200/50 text-[11px] text-slate-400">
          基于当前 {{ activeKeys.length }} 个就绪账号 Gemini 会话余量加权聚合 · 长周期配额防超限指示，自然周滚动重置
        </div>
      </div>
    </div>

    <!-- 账号详情抽屉 / 模态框 -->
    <div
      v-if="selectedKeyForDetails"
      class="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/40 backdrop-blur-xs p-4"
      data-testid="account-details-modal"
      @click.self="selectedKeyForDetails = null"
    >
      <div class="bg-white rounded-xl shadow-xl border border-slate-200 w-full max-w-lg overflow-hidden flex flex-col max-h-[90vh]">
        <!-- 头部 -->
        <div class="flex items-center justify-between px-5 py-4 border-b border-slate-100 bg-slate-50/70">
          <div class="flex items-center gap-2">
            <Icons name="key" size="18" class="text-amber-600" />
            <span class="font-bold text-slate-800 text-sm font-mono truncate max-w-[280px]">
              {{ selectedKeyForDetails.id }}
            </span>
            <span
              v-if="(keyTestResults[selectedKeyForDetails.id]?.usage || selectedKeyForDetails.usage)?.account_tier === 'pro'"
              class="px-2 py-0.5 rounded text-[11px] font-semibold bg-amber-100 text-amber-800 border border-amber-200"
            >
              ⭐ Pro 会员
            </span>
            <span
              v-else-if="(keyTestResults[selectedKeyForDetails.id]?.usage || selectedKeyForDetails.usage)?.account_tier === 'standard'"
              class="px-2 py-0.5 rounded text-[11px] font-semibold bg-sky-100 text-sky-800 border border-sky-200"
            >
              🔹 标准会员
            </span>
            <span
              v-else-if="(keyTestResults[selectedKeyForDetails.id]?.usage || selectedKeyForDetails.usage)?.account_tier === 'calibrating'"
              class="px-2 py-0.5 rounded text-[11px] font-semibold bg-indigo-100 text-indigo-800 border border-indigo-200"
            >
              🔄 校准中
            </span>
            <span
              v-else-if="(keyTestResults[selectedKeyForDetails.id]?.usage || selectedKeyForDetails.usage)?.account_tier === 'unknown'"
              class="px-2 py-0.5 rounded text-[11px] font-semibold bg-slate-100 text-slate-500 border border-slate-200"
            >
              ⏳ 待调用校准
            </span>
            <span
              v-else
              class="px-2 py-0.5 rounded text-[11px] font-semibold bg-slate-100 text-slate-600 border border-slate-200"
            >
              ⚪ 普通/免费
            </span>
          </div>
          <button
            class="text-slate-400 hover:text-slate-600 p-1 rounded-md transition-colors"
            @click="selectedKeyForDetails = null"
          >
            <Icons name="cross" size="16" />
          </button>
        </div>

        <!-- 正文 -->
        <div class="p-5 overflow-y-auto space-y-5 text-xs text-slate-600">
          <!-- 5h 即时窗口 -->
          <div class="p-3.5 rounded-lg bg-slate-50/80 border border-slate-100">
            <div class="flex items-center justify-between font-semibold text-slate-800 mb-2">
              <span class="flex items-center gap-1.5">
                <Icons name="zap" size="14" class="text-amber-500" />
                5小时爆发窗口 (Rolling 5h)
              </span>
              <span class="text-slate-400 font-mono text-[11px]">
                {{ (keyTestResults[selectedKeyForDetails.id]?.usage || selectedKeyForDetails.usage)?.window_5h.requests || 0 }} 次请求
              </span>
            </div>

            <div class="grid grid-cols-2 gap-3 mt-3">
              <div class="bg-white p-2.5 rounded border border-slate-100">
                <span class="text-slate-400 block text-[11px]">5h 已消耗 Tokens</span>
                <strong class="text-base font-mono text-slate-800 tabular-nums">
                  {{ ((keyTestResults[selectedKeyForDetails.id]?.usage || selectedKeyForDetails.usage)?.window_5h.total_tokens || 0).toLocaleString() }}
                </strong>
                <span class="text-[10px] text-slate-400 block mt-0.5">
                  输入: {{ ((keyTestResults[selectedKeyForDetails.id]?.usage || selectedKeyForDetails.usage)?.window_5h.prompt_tokens || 0).toLocaleString() }} · 输出: {{ ((keyTestResults[selectedKeyForDetails.id]?.usage || selectedKeyForDetails.usage)?.window_5h.completion_tokens || 0).toLocaleString() }}
                </span>
              </div>
              <div class="bg-white p-2.5 rounded border border-slate-100">
                <span class="text-slate-400 block text-[11px]">推算 5h 物理额度</span>
                <strong class="text-base font-mono text-emerald-600 tabular-nums">
                  {{ (keyTestResults[selectedKeyForDetails.id]?.usage || selectedKeyForDetails.usage)?.estimated_capacity_5h ? `~${((keyTestResults[selectedKeyForDetails.id]?.usage || selectedKeyForDetails.usage)?.estimated_capacity_5h || 0).toLocaleString()}` : '待拟合校准' }}
                </strong>
                <span class="text-[10px] text-slate-400 block mt-0.5">
                  预估剩余: {{ (keyTestResults[selectedKeyForDetails.id]?.usage || selectedKeyForDetails.usage)?.estimated_tokens_remaining_5h != null ? `~${((keyTestResults[selectedKeyForDetails.id]?.usage || selectedKeyForDetails.usage)?.estimated_tokens_remaining_5h || 0).toLocaleString()}` : '根据配额换算' }}
                </span>
              </div>
            </div>
          </div>

          <!-- 自然周窗口 -->
          <div class="p-3.5 rounded-lg bg-slate-50/80 border border-slate-100">
            <div class="flex items-center justify-between font-semibold text-slate-800 mb-2">
              <span class="flex items-center gap-1.5">
                <Icons name="activity" size="14" class="text-sky-500" />
                周度累计消耗 (Weekly Window)
              </span>
              <span class="text-slate-400 font-mono text-[11px]">
                {{ (keyTestResults[selectedKeyForDetails.id]?.usage || selectedKeyForDetails.usage)?.window_weekly.requests || 0 }} 次请求
              </span>
            </div>

            <div class="grid grid-cols-2 gap-3 mt-3">
              <div class="bg-white p-2.5 rounded border border-slate-100">
                <span class="text-slate-400 block text-[11px]">本周已消耗 Tokens</span>
                <strong class="text-base font-mono text-slate-800 tabular-nums">
                  {{ ((keyTestResults[selectedKeyForDetails.id]?.usage || selectedKeyForDetails.usage)?.window_weekly.total_tokens || 0).toLocaleString() }}
                </strong>
                <span class="text-[10px] text-slate-400 block mt-0.5">
                  输入: {{ ((keyTestResults[selectedKeyForDetails.id]?.usage || selectedKeyForDetails.usage)?.window_weekly.prompt_tokens || 0).toLocaleString() }} · 输出: {{ ((keyTestResults[selectedKeyForDetails.id]?.usage || selectedKeyForDetails.usage)?.window_weekly.completion_tokens || 0).toLocaleString() }}
                </span>
              </div>
              <div class="bg-white p-2.5 rounded border border-slate-100">
                <span class="text-slate-400 block text-[11px]">长效续航防触顶</span>
                <strong class="text-base font-mono text-slate-800 tabular-nums">
                  {{ extractKeyQuota(selectedKeyForDetails, keyTestResults[selectedKeyForDetails.id]).gemini.weeklyFraction > 0 ? `${Math.round(extractKeyQuota(selectedKeyForDetails, keyTestResults[selectedKeyForDetails.id]).gemini.weeklyFraction * 100)}% 剩余` : '已耗尽/冷却中' }}
                </strong>
                <span class="text-[10px] text-slate-400 block mt-0.5 truncate">
                  {{ extractKeyQuota(selectedKeyForDetails, keyTestResults[selectedKeyForDetails.id]).gemini.weeklyResetHint || '周期重置' }}
                </span>
              </div>
            </div>
          </div>

          <div class="text-[11px] text-slate-400 bg-slate-50 p-2.5 rounded border border-slate-100">
            💡 <strong>额度测算说明:</strong> 网关基于本地实时精准 Token 消耗计量与 Google Antigravity 官方配额跳变比例 (ΔTokens / ΔFraction) 进行动态回归拟合。当账号多次产生活跃调用后，将自适应校准物理容量并推断会员层级。
          </div>
        </div>

        <!-- 底部 -->
        <div class="px-5 py-3 border-t border-slate-100 bg-slate-50 flex justify-end">
          <UiButton size="sm" variant="secondary" @click="selectedKeyForDetails = null">
            关闭
          </UiButton>
        </div>
      </div>
    </div>
  </div>
</template>
