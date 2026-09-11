<script setup lang="ts">
import { ref, computed, watch, onMounted, onUnmounted } from 'vue';
import type { KeyView, KeyTestView, CreateKeyPayload } from '../../types/admin';
import Icons from '../ui/Icons.vue';
import UiButton from '../ui/UiButton.vue';
import UiBadge from '../ui/UiBadge.vue';
import UiTooltip from '../ui/UiTooltip.vue';
import UiCollapsible from '../ui/UiCollapsible.vue';
import { formatKeyState } from '../../utils/format';
import { toast } from '../../composables/useToast';

const props = defineProps<{
  providerName: string;
  keys: KeyView[];
  adminWriteEnabled: boolean;
  keyTestResults: Record<string, KeyTestView>;
  testingKeyIds: Set<string>;
  defaultExpanded?: boolean;
}>();

const emit = defineEmits<{
  (e: 'create', payload: CreateKeyPayload): Promise<void>;
  (e: 'delete', id: string): Promise<void>;
  (e: 'test-single', id: string): Promise<void>;
  (e: 'oauth-antigravity', providerName: string): void;
  (e: 'cooldown-expired'): void;
}>();

const isAntigravity = computed(() => props.providerName.toLowerCase().includes('antigravity'));

const isExpanded = ref(props.defaultExpanded ?? false);
const isAdding = ref(false);
const showAdvanced = ref(false);
const submitting = ref(false);
const formError = ref<string | null>(null);

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

const form = ref<CreateKeyPayload>({
  id: '',
  provider: props.providerName,
  api_key: '',
  priority: 1,
  weight: 10,
});

function openAddInline() {
  form.value = {
    id: `key-${Date.now().toString().slice(-4)}`,
    provider: props.providerName,
    api_key: '',
    priority: 1,
    weight: 10,
  };
  formError.value = null;
  showAdvanced.value = false;
  isExpanded.value = true;
  isAdding.value = true;
}

function cancelAdd() {
  isAdding.value = false;
  formError.value = null;
}

function formatQuotaPercent(fraction?: number | null): number {
  return Math.max(0, Math.min(100, Math.round((fraction ?? 0) * 100)));
}

function getQuotaProgressColor(fraction?: number | null): { bar: string; text: string; bg: string } {
  const f = fraction ?? 0;
  if (f > 0.3) {
    return { bar: 'bg-emerald-500', text: 'text-emerald-700', bg: 'bg-emerald-50' };
  } else if (f > 0.1) {
    return { bar: 'bg-amber-500', text: 'text-amber-700', bg: 'bg-amber-50' };
  } else {
    return { bar: 'bg-rose-500', text: 'text-rose-700', bg: 'bg-rose-50' };
  }
}

interface CompactBucketQuota {
  fraction: number;
  timeUntilReset: string;
}

interface CompactModelQuota {
  h5?: CompactBucketQuota;
  weekly?: CompactBucketQuota;
}

/**
 * Remaining cooldown seconds for a key. Dynamically decreases as time elapses
 * using the reactive nowMs tick. Prefers server-computed initial relative duration
 * adjusted by elapsed local time; falls back to absolute reset instant.
 */
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

/** Inline reset hint rendered right after the cooling badge. */
function cooldownResetHint(k: KeyView): string {
  const label = formatCooldownDuration(cooldownRemainingSecs(k));
  return label ? `${label}后解冻` : '';
}

function cooldownResetTooltip(k: KeyView): string {
  const label = formatCooldownDuration(cooldownRemainingSecs(k));
  const abs = k.cooldown_reset_at ? new Date(k.cooldown_reset_at).toLocaleString() : '';
  if (label && abs) {
    return `配额已耗尽，上游提示将于 ${abs} 重置（约 ${label}后解冻），冷冻期内不再向该密钥发送请求`;
  }
  if (label) return `配额已耗尽，约 ${label}后解冻，冷冻期内不再向该密钥发送请求`;
  if (abs) return `配额已耗尽，预计 ${abs} 重置，冷冻期内不再向该密钥发送请求`;
  return '配额已耗尽，冷却保护中';
}

function extractCompactQuotas(keyResult?: KeyTestView, isCoolingDown?: boolean): { gemini: CompactModelQuota; claude: CompactModelQuota } {
  const res: { gemini: CompactModelQuota; claude: CompactModelQuota } = {
    gemini: {},
    claude: {},
  };
  if (!keyResult) {
    if (isCoolingDown) {
      return {
        gemini: { h5: { fraction: 0, timeUntilReset: '冷却保护中' } },
        claude: { h5: { fraction: 0, timeUntilReset: '冷却保护中' } },
      };
    }
    return res;
  }

  // 1. Check quota_groups (from retrieveUserQuotaSummary)
  if (keyResult.quota_groups && keyResult.quota_groups.length > 0) {
    for (const group of keyResult.quota_groups) {
      const name = (group.display_name || '').toLowerCase();
      const target = name.includes('claude') || name.includes('gpt') || name.includes('3p')
        ? res.claude
        : res.gemini;

      for (const bucket of group.buckets || []) {
        const win = (bucket.window || '').toLowerCase();
        const bId = (bucket.bucket_id || '').toLowerCase();
        const bDesc = (bucket.description || '').toLowerCase();
        const bDisp = (bucket.display_name || '').toLowerCase();

        const isWeekly = win === 'weekly' || bId.includes('week') || bDesc.includes('week') || bDisp.includes('周') || bId.includes('7d');
        const is5h = win === '5h' || win.includes('5') || bId.includes('5h') || bId.includes('hour') || bDesc.includes('5') || bDisp.includes('5小时') || bDisp.includes('session');
        const qData: CompactBucketQuota = {
          fraction: bucket.remaining_fraction ?? 0,
          timeUntilReset: bucket.time_until_reset || bucket.reset_time_beijing || '已就绪',
        };

        if (isWeekly) {
          target.weekly = qData;
        } else if (is5h || !target.h5) {
          target.h5 = qData;
        } else {
          target.weekly = qData;
        }
      }
    }
  }

  // 2. Fallback to models in quota list (from fetchAvailableModels)
  if (keyResult.quota && keyResult.quota.length > 0) {
    for (const q of keyResult.quota) {
      const mId = q.model_id.toLowerCase();
      const isClaude = mId.includes('claude') || mId.includes('gpt') || mId.includes('sonnet') || mId.includes('opus');
      const target = isClaude ? res.claude : res.gemini;

      const qData: CompactBucketQuota = {
        fraction: q.remaining_fraction ?? 0,
        timeUntilReset: q.time_until_reset || q.reset_time_beijing || '已就绪',
      };

      // In flat model list, fetchAvailableModels represents the 5-hour rolling quota.
      // If the model ID or reset window contains weekly indicators, record as weekly; otherwise 5h.
      const isWeeklyModel = mId.includes('week') || mId.includes('7d') || (q.time_until_reset && (q.time_until_reset.includes('天') || q.time_until_reset.includes('d')));
      if (isWeeklyModel) {
        if (!target.weekly || target.weekly.fraction > (q.remaining_fraction ?? 0)) {
          target.weekly = qData;
        }
      } else {
        if (!target.h5 || target.h5.fraction > (q.remaining_fraction ?? 0)) {
          target.h5 = qData;
        }
      }
    }
  }

  // 3. 即使额度为0，或探测响应成功但某些窗口未下发，只要已探测即保证有 5h 默认展示，确保进度条稳定呈现
  if (!res.gemini.h5) {
    res.gemini.h5 = { fraction: isCoolingDown ? 0 : 0, timeUntilReset: isCoolingDown ? '冷却保护中' : '已就绪' };
  }
  if (!res.claude.h5) {
    res.claude.h5 = { fraction: isCoolingDown ? 0 : 0, timeUntilReset: isCoolingDown ? '冷却保护中' : '已就绪' };
  }

  return res;
}

async function handleSubmit() {
  if (!props.adminWriteEnabled) return;
  const id = form.value.id.trim();
  const rawKey = form.value.api_key.trim();
  if (!id) {
    formError.value = '请输入密钥标识';
    return;
  }
  if (!rawKey) {
    formError.value = '请输入 API Key 明文';
    return;
  }

  submitting.value = true;
  formError.value = null;
  try {
    await emit('create', {
      ...form.value,
      id,
      api_key: rawKey,
      provider: props.providerName,
    });
    isAdding.value = false;
  } catch (err: unknown) {
    formError.value = err instanceof Error ? err.message : String(err);
  } finally {
    submitting.value = false;
  }
}

async function handleDelete(id: string) {
  if (!props.adminWriteEnabled) return;
  const confirmed = await toast.confirm({
    title: '移除密钥',
    message: `确定移除密钥 "${id}" 吗？该操作将热同步连接池。`,
    confirmText: '确认移除',
    cancelText: '取消',
    variant: 'destructive',
  });
  if (!confirmed) {
    return;
  }
  try {
    await emit('delete', id);
    toast.success(`密钥 "${id}" 已成功移除`);
  } catch (err: unknown) {
    toast.error(`删除失败: ${err instanceof Error ? err.message : String(err)}`);
  }
}
const isRefreshingAll = ref(false);

const isRefreshingAny = computed(() => {
  if (isRefreshingAll.value) return true;
  return props.keys.some((k) => props.testingKeyIds.has(k.id));
});

async function handleRefreshAllQuotas() {
  if (!props.adminWriteEnabled || isRefreshingAny.value || props.keys.length === 0) return;
  isRefreshingAll.value = true;
  try {
    const promises = props.keys.map((k) => emit('test-single', k.id));
    await Promise.allSettled(promises);
    toast.success('已刷新全部账号配额用量');
  } catch (err: unknown) {
    toast.error(`刷新失败: ${err instanceof Error ? err.message : String(err)}`);
  } finally {
    isRefreshingAll.value = false;
  }
}
</script>

<template>
  <div class="space-y-2">
    <!-- 标题与快捷添加按钮 (支持独立折叠) -->
    <div
      class="flex items-center justify-between pb-1 cursor-pointer select-none"
      @click="isExpanded = !isExpanded"
    >
      <div class="flex items-center gap-2 text-sm font-semibold text-slate-800">
        <Icons name="key" size="14" class="text-amber-500" />
        密钥 ({{ keys.length }})
        <UiTooltip content="网关向该厂商转发请求所使用的 API 密钥池，支持多 Key 负载均衡">
          <Icons name="info" size="13" class="text-slate-400 hover:text-slate-600 cursor-pointer" />
        </UiTooltip>
      </div>

      <div class="flex items-center gap-1.5" @click.stop>
        <!-- Antigravity 统一刷新配额用量按钮 (仅在有密钥时提供) -->
        <UiTooltip v-if="isAntigravity && keys.length > 0" :content="isRefreshingAny ? '正在刷新配额用量...' : '刷新全部账号配额用量'">
          <UiButton
            variant="ghost"
            size="icon"
            aria-label="刷新全部账号配额用量"
            :disabled="!adminWriteEnabled || isRefreshingAny"
            data-testid="refresh-antigravity-quota-btn"
            class="text-amber-600 hover:text-amber-700 hover:bg-amber-50/60"
            @click="handleRefreshAllQuotas"
          >
            <Icons
              name="refresh"
              size="14"
              :class="isRefreshingAny ? 'animate-spin text-amber-600' : ''"
            />
          </UiButton>
        </UiTooltip>

        <UiButton
          v-if="isAntigravity"
          variant="ghost"
          size="sm"
          :disabled="!adminWriteEnabled"
          data-testid="add-antigravity-key-btn"
          class="text-amber-600 hover:text-amber-700 hover:bg-amber-50/60 font-medium px-2.5 py-1 text-[13px]"
          @click="emit('oauth-antigravity', props.providerName)"
        >
          <Icons name="zap" size="14" />
          授权账号
        </UiButton>
        <UiButton
          variant="ghost"
          size="sm"
          :disabled="!adminWriteEnabled || isAdding"
          data-testid="add-key-btn"
          class="text-indigo-600 hover:text-indigo-700 hover:bg-indigo-50/60 font-medium px-2.5 py-1 text-[13px]"
          @click="openAddInline"
        >
          <Icons name="plus" size="14" />
          密钥
        </UiButton>
        <UiButton
          variant="ghost"
          size="icon"
          :aria-label="isExpanded ? '收起密钥列表' : '展开密钥列表'"
          class="text-slate-400 hover:text-slate-600"
          data-testid="toggle-keys-btn"
          @click="isExpanded = !isExpanded"
        >
          <Icons :name="isExpanded ? 'chevron-down' : 'chevron-right'" size="15" />
        </UiButton>
      </div>
    </div>

    <!-- 可独立折叠的内容容器 (默认折叠) -->
    <UiCollapsible :open="isExpanded">
      <div class="pt-2 space-y-2">
        <!-- 行内平滑展开新建表单 -->
        <UiCollapsible :open="isAdding">
          <div class="p-4 bg-slate-50/90 rounded-lg border border-slate-200/80 mb-3 text-sm space-y-3">
            <div class="flex items-center justify-between">
              <span class="font-semibold text-slate-900 text-sm">新建密钥</span>
              <button
                type="button"
                class="text-slate-400 hover:text-slate-600 cursor-pointer p-0.5"
                @click="cancelAdd"
              >
                <Icons name="cross" size="14" />
              </button>
            </div>

            <div v-if="formError" class="p-2.5 bg-rose-50 text-rose-600 rounded-md text-sm font-medium">
              {{ formError }}
            </div>

            <form class="space-y-3" @submit.prevent="handleSubmit">
              <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
                <div>
                  <label class="block text-slate-700 font-medium mb-1.5 text-[13px]">Key 标识 *</label>
                  <input
                    v-model="form.id"
                    type="text"
                    placeholder="例如: key-01"
                    required
                    class="w-full bg-white border border-slate-200/80 rounded-md px-3 py-2 text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500"
                    data-testid="key-id-input"
                  />
                </div>

                <div>
                  <label class="block text-slate-700 font-medium mb-1.5 text-[13px]">API Key 明文 *</label>
                  <input
                    v-model="form.api_key"
                    type="password"
                    placeholder="sk-..."
                    required
                    class="w-full bg-white border border-slate-200/80 rounded-md px-3 py-2 text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 focus:border-indigo-500"
                    data-testid="key-secret-input"
                  />
                </div>
              </div>

              <!-- 高级选项折叠 (权重/优先级) -->
              <div class="pt-0.5">
                <button
                  type="button"
                  class="text-[13px] font-semibold text-slate-700 hover:text-indigo-600 inline-flex items-center gap-1 cursor-pointer py-1 select-none"
                  @click="showAdvanced = !showAdvanced"
                >
                  <Icons :name="showAdvanced ? 'chevron-down' : 'chevron-right'" size="13" />
                  高级调度参数 (权重与优先级)
                </button>

                <UiCollapsible :open="showAdvanced">
                  <div class="grid grid-cols-2 gap-3 pt-2 p-3 bg-slate-100/70 rounded-md border border-slate-200/60 mt-1">
                    <div>
                      <label class="block text-xs text-slate-600 mb-1 font-medium">优先级 (数值越小越优先)</label>
                      <input
                        v-model.number="form.priority"
                        type="number"
                        min="0"
                        class="w-full bg-white border border-slate-200 rounded-md px-3 py-1.5 text-sm text-slate-900"
                        data-testid="key-priority-input"
                      />
                    </div>
                    <div>
                      <label class="block text-xs text-slate-600 mb-1 font-medium">权重 (轮询比重)</label>
                      <input
                        v-model.number="form.weight"
                        type="number"
                        min="1"
                        class="w-full bg-white border border-slate-200 rounded-md px-3 py-1.5 text-sm text-slate-900"
                        data-testid="key-weight-input"
                      />
                    </div>
                  </div>
                </UiCollapsible>
              </div>

              <div class="flex items-center justify-end gap-2 pt-2 border-t border-slate-200/60">
                <UiButton variant="ghost" size="sm" @click="cancelAdd">
                  取消
                </UiButton>
                <UiButton
                  type="submit"
                  size="sm"
                  :disabled="submitting || !adminWriteEnabled"
                  data-testid="submit-key-btn"
                >
                  {{ submitting ? '保存中...' : '保存密钥' }}
                </UiButton>
              </div>
            </form>
          </div>
        </UiCollapsible>

        <!-- 密钥条目列表 (彻底去除第三层边框与背景，仅靠微交互区分) -->
        <div v-if="keys.length === 0" class="py-3 text-center text-sm text-slate-500 bg-transparent border-none rounded-md">
          暂未配置密钥，点击上方「+ 密钥」快速添加
        </div>

        <div v-else class="space-y-1">
          <div
            v-for="k in keys"
            :key="k.id"
            class="bg-transparent hover:bg-white/35 rounded-lg border-none transition-colors text-sm p-2.5 space-y-2"
            data-testid="key-row"
          >
            <div class="flex items-center justify-between">
              <div class="flex items-center gap-2.5 min-w-0">
                <span class="font-mono text-slate-900 font-semibold text-sm truncate">{{ k.id }}</span>
                <span class="font-mono text-slate-500 text-[13px]">{{ k.masked_key }}</span>
                <!-- 仅在非 active 状态（如 cooling_down / disabled）下显示状态徽标，正常可用时不显示“就绪” -->
                <UiBadge
                  v-if="k.state !== 'active'"
                  :variant="k.state === 'cooling_down' ? 'warning' : 'destructive'"
                >
                  {{ formatKeyState(k.state) }}
                </UiBadge>
                <!-- 冷冻徽标后直出上游告知的重置/解冻时间：冷冻期内不再向该密钥发请求 -->
                <UiTooltip
                  v-if="k.state === 'cooling_down' && cooldownResetHint(k)"
                  :content="cooldownResetTooltip(k)"
                >
                  <span
                    class="text-[11px] font-medium text-amber-700/90 whitespace-nowrap select-none"
                    data-testid="key-cooldown-reset"
                  >
                    {{ cooldownResetHint(k) }}
                  </span>
                </UiTooltip>
              </div>

              <div class="flex items-center gap-3 shrink-0">
                <!-- 行内双进度条 (Gemini / Claude 的 5h 与周用量)，显示在刷新按钮前方 -->
                <div
                  v-if="isAntigravity"
                  class="flex items-center gap-2"
                  data-testid="antigravity-quota-container"
                >
                  <!-- 未探测且非冷却状态时展示占位提示 -->
                  <div
                    v-if="!keyTestResults[k.id] && k.state !== 'cooling_down'"
                    class="text-[11px] text-slate-400 font-normal select-none"
                  >
                    点击上方刷新查看用量
                  </div>

                  <!-- 探测结果或冷却保护状态：Gemini 胶囊双进度条 (G: 5h / 周) -->
                  <div
                    v-if="keyTestResults[k.id] || k.state === 'cooling_down'"
                    class="flex items-center gap-1.5 px-2 py-1 bg-white/60 hover:bg-white/90 rounded-md border border-slate-200/80 text-[11px] shadow-2xs transition-colors"
                    data-testid="quota-capsule-gemini"
                  >
                    <UiTooltip content="Gemini 系列模型">
                      <span class="font-bold text-slate-700 tracking-tight cursor-help">G</span>
                    </UiTooltip>
                    <!-- Gemini 5h -->
                    <UiTooltip
                      :content="`Gemini 5小时用量剩余 ${formatQuotaPercent(extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').gemini.h5?.fraction)}% (${extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').gemini.h5?.timeUntilReset || '已就绪'})`"
                    >
                      <div class="flex items-center gap-1 cursor-default">
                        <span class="text-[10px] text-slate-500 font-mono">5h</span>
                        <div class="w-12 bg-slate-200 rounded-full h-1 overflow-hidden">
                          <div
                            class="h-full rounded-full transition-all duration-300"
                            :class="getQuotaProgressColor(extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').gemini.h5?.fraction).bar"
                            :style="{ width: `${formatQuotaPercent(extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').gemini.h5?.fraction)}%` }"
                          />
                        </div>
                        <span
                          class="font-mono text-[10px] font-semibold"
                          :class="getQuotaProgressColor(extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').gemini.h5?.fraction).text"
                        >
                          {{ formatQuotaPercent(extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').gemini.h5?.fraction) }}%
                        </span>
                      </div>
                    </UiTooltip>

                    <!-- Gemini 周用量 (若无真实周数据，默认展示 100% / 未受限状态) -->
                    <span class="text-slate-300">|</span>
                    <UiTooltip
                      :content="extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').gemini.weekly
                        ? `Gemini 周用量剩余 ${formatQuotaPercent(extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').gemini.weekly?.fraction)}% (${extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').gemini.weekly?.timeUntilReset})`
                        : 'Gemini 周配额未达阈值或已就绪 (100%)'"
                    >
                      <div class="flex items-center gap-1 cursor-default">
                        <span class="text-[10px] text-slate-500 font-mono">周</span>
                        <div class="w-12 bg-slate-200 rounded-full h-1 overflow-hidden">
                          <div
                            class="h-full rounded-full transition-all duration-300"
                            :class="getQuotaProgressColor(extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').gemini.weekly?.fraction ?? 1.0).bar"
                            :style="{ width: `${formatQuotaPercent(extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').gemini.weekly?.fraction ?? 1.0)}%` }"
                          />
                        </div>
                        <span
                          class="font-mono text-[10px] font-semibold"
                          :class="getQuotaProgressColor(extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').gemini.weekly?.fraction ?? 1.0).text"
                        >
                          {{ formatQuotaPercent(extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').gemini.weekly?.fraction ?? 1.0) }}%
                        </span>
                      </div>
                    </UiTooltip>
                  </div>

                  <!-- 探测结果或冷却保护状态：Claude 胶囊双进度条 (C: 5h / 周) -->
                  <div
                    v-if="keyTestResults[k.id] || k.state === 'cooling_down'"
                    class="flex items-center gap-1.5 px-2 py-1 bg-white/60 hover:bg-white/90 rounded-md border border-slate-200/80 text-[11px] shadow-2xs transition-colors"
                    data-testid="quota-capsule-claude"
                  >
                    <UiTooltip content="Claude 系列模型">
                      <span class="font-bold text-slate-700 tracking-tight cursor-help">C</span>
                    </UiTooltip>
                    <!-- Claude 5h -->
                    <UiTooltip
                      :content="`Claude 5小时用量剩余 ${formatQuotaPercent(extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').claude.h5?.fraction)}% (${extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').claude.h5?.timeUntilReset || '已就绪'})`"
                    >
                      <div class="flex items-center gap-1 cursor-default">
                        <span class="text-[10px] text-slate-500 font-mono">5h</span>
                        <div class="w-12 bg-slate-200 rounded-full h-1 overflow-hidden">
                          <div
                            class="h-full rounded-full transition-all duration-300"
                            :class="getQuotaProgressColor(extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').claude.h5?.fraction).bar"
                            :style="{ width: `${formatQuotaPercent(extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').claude.h5?.fraction)}%` }"
                          />
                        </div>
                        <span
                          class="font-mono text-[10px] font-semibold"
                          :class="getQuotaProgressColor(extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').claude.h5?.fraction).text"
                        >
                          {{ formatQuotaPercent(extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').claude.h5?.fraction) }}%
                        </span>
                      </div>
                    </UiTooltip>

                    <!-- Claude 周用量 (若无真实周数据，默认展示 100% / 未受限状态) -->
                    <span class="text-slate-300">|</span>
                    <UiTooltip
                      :content="extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').claude.weekly
                        ? `Claude 周用量剩余 ${formatQuotaPercent(extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').claude.weekly?.fraction)}% (${extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').claude.weekly?.timeUntilReset})`
                        : 'Claude 周配额未达阈值或已就绪 (100%)'"
                    >
                      <div class="flex items-center gap-1 cursor-default">
                        <span class="text-[10px] text-slate-500 font-mono">周</span>
                        <div class="w-12 bg-slate-200 rounded-full h-1 overflow-hidden">
                          <div
                            class="h-full rounded-full transition-all duration-300"
                            :class="getQuotaProgressColor(extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').claude.weekly?.fraction ?? 1.0).bar"
                            :style="{ width: `${formatQuotaPercent(extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').claude.weekly?.fraction ?? 1.0)}%` }"
                          />
                        </div>
                        <span
                          class="font-mono text-[10px] font-semibold"
                          :class="getQuotaProgressColor(extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').claude.weekly?.fraction ?? 1.0).text"
                        >
                          {{ formatQuotaPercent(extractCompactQuotas(keyTestResults[k.id], k.state === 'cooling_down').claude.weekly?.fraction ?? 1.0) }}%
                        </span>
                      </div>
                    </UiTooltip>
                  </div>
                </div>

                <!-- 非 Antigravity 专享通用拨测按钮 (Antigravity 已在标题行统一刷新) -->
                <UiTooltip v-if="!isAntigravity" content="测试密钥连通性">
                  <UiButton
                    variant="ghost"
                    size="icon"
                    :aria-label="`测试密钥 ${k.id} 连通性`"
                    :disabled="testingKeyIds.has(k.id) || !adminWriteEnabled"
                    :data-testid="`test-key-${k.id}`"
                    class="text-slate-500 hover:text-amber-600 hover:bg-amber-50"
                    @click="emit('test-single', k.id)"
                  >
                    <Icons
                      name="refresh"
                      size="14"
                      :class="testingKeyIds.has(k.id) ? 'animate-spin text-amber-600' : ''"
                    />
                  </UiButton>
                </UiTooltip>

                <!-- 删除纯图标按钮 -->
                <UiTooltip content="移除此密钥">
                  <UiButton
                    variant="ghost"
                    size="icon"
                    :aria-label="`删除密钥 ${k.id}`"
                    :disabled="!adminWriteEnabled"
                    data-testid="delete-key-btn"
                    class="text-slate-400 hover:text-rose-600 hover:bg-rose-50"
                    @click="handleDelete(k.id)"
                  >
                    <Icons name="trash" size="14" />
                  </UiButton>
                </UiTooltip>
              </div>
            </div>
          </div>
        </div>
      </div>
    </UiCollapsible>
  </div>
</template>
