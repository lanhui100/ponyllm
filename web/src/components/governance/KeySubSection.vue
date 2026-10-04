<script setup lang="ts">
import { ref, computed, watch, onMounted, onUnmounted } from 'vue';
import type { KeyView, KeyTestView, CreateKeyPayload, UpdateKeyPayload } from '../../types/admin';
import Icons from '../ui/Icons.vue';
import UiButton from '../ui/UiButton.vue';
import UiBadge from '../ui/UiBadge.vue';
import UiTooltip from '../ui/UiTooltip.vue';
import UiCollapsible from '../ui/UiCollapsible.vue';
import { formatKeyState } from '../../utils/format';
import { toast } from '../../composables/useToast';
import {
  extractUnifiedGeminiQuota,
  isAntigravityScope,
  recordCooldownSnapshots,
  cooldownRemainingSecsFrom,
  formatCooldownDuration,
  isLockBusyResult,
  type CooldownSnapshot,
} from '../../utils/antigravityQuota';
import { isQuotaResultFresh } from '../../composables/useAdminConfig';

const props = defineProps<{
  providerName: string;
  providerDefaultProtocol?: string | null;
  keys: KeyView[];
  adminWriteEnabled: boolean;
  keyTestResults: Record<string, KeyTestView>;
  testingKeyIds: Set<string>;
  defaultExpanded?: boolean;
}>();

const emit = defineEmits<{
  (e: 'create', payload: CreateKeyPayload): Promise<void>;
  (e: 'update', id: string, payload: UpdateKeyPayload): Promise<void>;
  (e: 'delete', id: string): Promise<void>;
  (e: 'test-single', id: string): Promise<void>;
  (e: 'oauth-antigravity', providerName: string): void;
  (e: 'reauthorize', providerName: string, keyId: string): void;
  (e: 'cooldown-expired'): void;
}>();

/** 统一 Antigravity 归属判定（与 ProviderCard 同口径，default_protocol 优先）。 */
const isAntigravity = computed(() =>
  isAntigravityScope(props.providerName, props.providerDefaultProtocol),
);

const isExpanded = ref(props.defaultExpanded ?? false);
const isAdding = ref(false);
const showAdvanced = ref(false);
const submitting = ref(false);
const formError = ref<string | null>(null);

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

const form = ref<CreateKeyPayload>({
  id: '',
  provider: props.providerName,
  api_key: '',
  priority: 1,
  weight: 10,
});

function openAddInline() {
  const nextPriority = props.keys.length > 0 ? Math.max(...props.keys.map((k) => k.priority || 0)) + 1 : 1;
  form.value = {
    id: `key-${Date.now().toString().slice(-4)}`,
    provider: props.providerName,
    api_key: '',
    priority: nextPriority,
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

function formatKeyDisplay(k: KeyView, isAntigravityProvider: boolean): { title: string; subtitle?: string } {
  if (isAntigravityProvider) {
    let email = k.id;
    if (email.startsWith('ag-')) {
      email = email.slice(3);
    }
    return {
      title: email,
      subtitle: undefined,
    };
  }
  return {
    title: k.id,
    subtitle: k.masked_key,
  };
}

function formatQuotaPercent(fraction?: number | null): number | null {
  if (fraction == null) return null;
  return Math.max(0, Math.min(100, Math.round(fraction * 100)));
}

function formatQuotaPercentText(fraction?: number | null): string {
  const v = formatQuotaPercent(fraction);
  return v == null ? '--' : `${v}%`;
}

function quotaBarWidth(fraction?: number | null): string {
  const v = formatQuotaPercent(fraction);
  return v == null ? '0%' : `${v}%`;
}

/** 未知额度条走中性灰，不再伪装红/绿。 */
function getQuotaProgressColor(fraction?: number | null): { bar: string; text: string; bg: string } {
  if (fraction == null) {
    return { bar: 'bg-slate-300', text: 'text-slate-400', bg: 'bg-slate-50' };
  }
  const f = fraction;
  if (f > 0.3) {
    return { bar: 'bg-emerald-500', text: 'text-emerald-700', bg: 'bg-emerald-50' };
  } else if (f > 0.1) {
    return { bar: 'bg-amber-500', text: 'text-amber-700', bg: 'bg-amber-50' };
  } else {
    return { bar: 'bg-rose-500', text: 'text-rose-700', bg: 'bg-rose-50' };
  }
}

interface CompactBucketQuota {
  fraction: number | null;
  timeUntilReset: string;
}

interface CompactModelQuota {
  h5?: CompactBucketQuota | null;
  weekly?: CompactBucketQuota | null;
  isLockBusy?: boolean;
}

/** 探测结果是否新鲜：过期缓存渲染为"未知"占位，不再伪装 0%/100%。 */
function hasFreshProbe(keyId: string): boolean {
  return isQuotaResultFresh(keyId);
}

/** Inline reset hint rendered right after the cooling badge. */
function cooldownResetHint(k: KeyView): string {
  const label = formatCooldownDuration(cooldownRemainingSecs(k));
  return label ? `${label}后解冻` : '';
}

function cooldownResetTooltip(k: KeyView): string {
  if (k.cooldown_reason === 'eligibility') {
    const label = formatCooldownDuration(cooldownRemainingSecs(k));
    const abs = k.cooldown_reset_at ? new Date(k.cooldown_reset_at).toLocaleString() : '';
    const why = (k.error_message || '上游判定账号无该产品资格（not eligible）').slice(0, 300);
    const when = label
      ? `约 ${label} 后自动解冻`
      : abs ? `预计 ${abs} 自动解冻` : '冻结数日后到期，等待上游状态变化';
    return `上游资格受限（not eligible），账号被冻结跳过，请求自动路由到其它账号。${when}。\n原因: ${why}`;
  }
  const label = formatCooldownDuration(cooldownRemainingSecs(k));
  const abs = k.cooldown_reset_at ? new Date(k.cooldown_reset_at).toLocaleString() : '';
  if (label && abs) {
    return `配额已耗尽，上游提示将于 ${abs} 重置（约 ${label}后解冻），冷冻期内不再向该密钥发送请求`;
  }
  if (label) return `配额已耗尽，约 ${label}后解冻，冷冻期内不再向该密钥发送请求`;
  if (abs) return `配额已耗尽，预计 ${abs} 重置，冷冻期内不再向该密钥发送请求`;
  return '配额已耗尽，冷却保护中';
}

function getDisabledStatusInfo(k: KeyView): { label: string; variant: 'destructive' | 'warning' | 'purple' } {
  const reason = k.disabled_reason?.toLowerCase() || '';
  if (reason.includes('validation_required') || reason.includes('verify your account')) {
    return { label: '需验证', variant: 'warning' };
  }
  if (reason.includes('invalid_grant') || reason.includes('token has been expired') || reason.includes('revoked')) {
    return { label: '凭据失效', variant: 'destructive' };
  }
  if (reason.includes('policy') || reason.includes('terms of service') || reason.includes('suspended')) {
    return { label: '违规停用', variant: 'destructive' };
  }
  return { label: '已禁用', variant: 'destructive' };
}

function disabledReasonTooltip(k: KeyView): string {
  const reason = k.disabled_reason?.trim();
  const noun = isAntigravity.value ? '账号' : '密钥';
  if (!reason) {
    return `该${noun}已被网关永久禁用。可能原因：OAuth 凭据失效（invalid_grant）、账号密码重置、授权被撤销或账号触犯上游服务条款（PolicyViolation）。`;
  }
  const lower = reason.toLowerCase();
  let explanation = '';
  if (lower.includes('validation_required') || lower.includes('verify your account')) {
    explanation = 'Google 要求对该账号进行安全验证 (VALIDATION_REQUIRED)。请登录该 Google 账号完成验证或重新授权。';
  } else if (lower.includes('invalid_grant')) {
    explanation = '上游判定 OAuth 授权失效 (invalid_grant)。可能是 Google 授权被撤销、密码被修改、Refresh Token 过期或账号已被上游风控冻结。';
  } else if (lower.includes('terms of service') || lower.includes('policy') || lower.includes('suspended')) {
    explanation = `上游判定${noun}违规或停用 (PolicyViolation)。触犯了服务条款或已被上游封禁。`;
  } else if (/\b(auth|unauthorized|401)\b/i.test(lower) || lower.includes('invalid key') || lower.includes('invalid_api_key')) {
    explanation = `身份鉴权失败，该${noun}无效或已失效。`;
  } else {
    explanation = `该${noun}已被永久禁用，不再分配流量。`;
  }
  const cleanReason = reason.length > 300 ? `${reason.slice(0, 297)}...` : reason;
  return `${explanation}\n\n具体原因: ${cleanReason}`;
}

function extractCompactQuotas(keyId: string, keyResult?: KeyTestView, isCoolingDown?: boolean): { gemini: CompactModelQuota } {
  const res: { gemini: CompactModelQuota } = {
    gemini: {},
  };
  // 无结果：冷却态画 0% 冷却条（后端已判冷），非冷却渲染"点击刷新"占位。
  if (!keyResult) {
    if (isCoolingDown) {
      return { gemini: { h5: { fraction: 0, timeUntilReset: '冷却保护中' } } };
    }
    return res;
  }

  // 跨节点锁冲突 (lock_busy)：瞬态锁同步中，不计为硬性探测失败
  if (isLockBusyResult(keyResult)) {
    if (isCoolingDown) {
      return { gemini: { h5: { fraction: 0, timeUntilReset: '冷却保护中' } } };
    }
    return { gemini: { isLockBusy: true } };
  }

  // 过期缓存：冷却态画 0% 冷却条，非冷却视为未知占位，提示手动刷新。
  if (!hasFreshProbe(keyId)) {
    if (isCoolingDown) {
      return { gemini: { h5: { fraction: 0, timeUntilReset: '冷却保护中' } } };
    }
    return res;
  }
  // 探测失败（无 quota 数据且 success=false）：冷却态画 0% 冷却条，非冷却渲染"探测失败"占位。
  const u0 = extractUnifiedGeminiQuota(keyResult);
  if (!u0.h5 && !u0.weekly && keyResult.success === false) {
    if (isCoolingDown) {
      return { gemini: { h5: { fraction: 0, timeUntilReset: '冷却保护中' } } };
    }
    return res;
  }
  const u = u0;
  if (u.h5) {
    res.gemini.h5 = { fraction: u.h5.fraction, timeUntilReset: u.h5.timeUntilReset };
  } else if (isCoolingDown) {
    res.gemini.h5 = { fraction: 0, timeUntilReset: '冷却保护中' };
  }
  // 周缺席（weeklySeen=false）=> 保持 undefined，UI 渲染"未下发"中性占位，不再按 100% 绿条。
  if (u.weekly) {
    res.gemini.weekly = { fraction: u.weekly.fraction, timeUntilReset: u.weekly.timeUntilReset };
  }

  return res;
}

// 缓存单次渲染中各 key 的配额画像，避免模板单次渲染重复调用 13 次造成额外开销
const compactQuotasByKey = computed(() => {
  const map = new Map<string, { gemini: CompactModelQuota }>();
  for (const k of props.keys) {
    map.set(k.id, extractCompactQuotas(k.id, props.keyTestResults[k.id], k.state === 'cooling_down'));
  }
  return map;
});

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

const editingKeyId = ref<string | null>(null);
const editPriority = ref(1);
const editWeight = ref(10);
const editSaving = ref(false);

function startEditKey(k: KeyView) {
  if (!props.adminWriteEnabled) return;
  editingKeyId.value = k.id;
  editPriority.value = k.priority;
  editWeight.value = k.weight;
}

function cancelEditKey() {
  editingKeyId.value = null;
}

async function saveEditKey(id: string) {
  if (!props.adminWriteEnabled) return;
  editSaving.value = true;
  try {
    await emit('update', id, {
      priority: editPriority.value,
      weight: editWeight.value,
    });
    editingKeyId.value = null;
    toast.success('密钥调度参数已更新');
  } catch (err: unknown) {
    toast.error(`更新失败: ${err instanceof Error ? err.message : String(err)}`);
  } finally {
    editSaving.value = false;
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
  // 防风控：串行逐个探测 + 800ms 间隔，避免 N 并行同时打 Google 上游配额接口。
  let ok = 0;
  let failed = 0;
  try {
    for (const k of props.keys) {
      try {
        await emit('test-single', k.id);
        ok += 1;
      } catch {
        failed += 1;
      }
      await new Promise((resolve) => setTimeout(resolve, 800));
    }
    if (failed === 0) {
      toast.success(`已刷新全部账号配额用量（${ok} 个）`);
    } else {
      toast.warning(`配额刷新部分成功：${ok} 成功 / ${failed} 失败，失败账号可单独重试`);
    }
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
      class="flex items-center justify-between min-h-[32px] pb-1 cursor-pointer select-none"
      @click="isExpanded = !isExpanded"
    >
      <div class="flex items-center gap-2 text-sm font-semibold text-slate-800 leading-6">
        <Icons name="key" size="15" class="text-amber-600" />
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
                <span
                  class="font-mono text-slate-900 font-semibold text-sm truncate max-w-[280px]"
                  :title="formatKeyDisplay(k, isAntigravity).title"
                >
                  {{ formatKeyDisplay(k, isAntigravity).title }}
                </span>
                <span
                  v-if="formatKeyDisplay(k, isAntigravity).subtitle"
                  class="font-mono text-slate-500 text-[13px]"
                >
                  {{ formatKeyDisplay(k, isAntigravity).subtitle }}
                </span>
                <!-- 优先级与权重徽标 / 编辑触发 -->
                <div class="flex items-center gap-1.5 text-xs text-slate-500 font-mono">
                  <UiTooltip content="优先级: 数值越小越优先，1耗尽自动故障顺延至2">
                    <span
                      class="px-1.5 py-0.5 rounded bg-slate-100/80 hover:bg-slate-200/80 text-slate-700 font-semibold cursor-pointer border border-slate-200/60"
                      :data-testid="`key-priority-badge-${k.id}`"
                      @click="startEditKey(k)"
                    >
                      P{{ k.priority }}
                    </span>
                  </UiTooltip>
                  <UiTooltip content="权重 (轮询比重)">
                    <span
                      class="px-1.5 py-0.5 rounded bg-slate-100/80 hover:bg-slate-200/80 text-slate-600 cursor-pointer border border-slate-200/60"
                      :data-testid="`key-weight-badge-${k.id}`"
                      @click="startEditKey(k)"
                    >
                      W{{ k.weight }}
                    </span>
                  </UiTooltip>
                </div>

                <!-- 仅在非 active 状态（如 cooling_down / disabled）下显示状态徽标，正常可用时不显示“就绪” -->
                <div v-if="k.state === 'disabled'" class="inline-flex items-center gap-1.5">
                  <!-- 禁用徽标本身包裹 Tooltip 展示具体禁用原因，去除独立 info 图标 -->
                  <UiTooltip
                    :content="disabledReasonTooltip(k)"
                    wrap
                  >
                    <span class="inline-flex cursor-help" :data-testid="`key-disabled-tooltip-trigger-${k.id}`">
                      <UiBadge
                        :variant="getDisabledStatusInfo(k).variant"
                        :data-testid="`key-state-badge-${k.id}`"
                      >
                        {{ getDisabledStatusInfo(k).label }}
                      </UiBadge>
                    </span>
                  </UiTooltip>

                  <!-- 重新授权纯图标按钮：无背景色，更符合语义的 key 凭据图标，hover 展示 Tooltip -->
                  <UiTooltip v-if="isAntigravity" content="重新授权该账号（OAuth）">
                    <button
                      type="button"
                      class="inline-flex items-center justify-center p-0.5 text-amber-600 hover:text-amber-700 transition-colors cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed focus:outline-hidden"
                      :disabled="!adminWriteEnabled"
                      :data-testid="`reauthorize-key-${k.id}`"
                      aria-label="重新授权"
                      @click="emit('reauthorize', props.providerName, k.id)"
                    >
                      <Icons name="key" size="14" class="stroke-[2.2]" />
                    </button>
                  </UiTooltip>
                </div>

                <UiBadge
                  v-else-if="k.state === 'cooling_down'"
                  :variant="k.cooldown_reason === 'eligibility' ? 'destructive' : 'warning'"
                  :data-testid="`key-state-badge-${k.id}`"
                >
                  {{ k.cooldown_reason === 'eligibility' ? '资格受限' : formatKeyState(k.state) }}
                </UiBadge>
                <!-- 冷冻徽标后直出上游告知的重置/解冻时间：冷冻期内不再向该密钥发请求 -->
                <UiTooltip
                  v-if="k.state === 'cooling_down' && cooldownResetHint(k)"
                  :content="cooldownResetTooltip(k)"
                >
                  <span
                    class="text-[11px] font-medium whitespace-nowrap select-none"
                    :class="k.cooldown_reason === 'eligibility' ? 'text-rose-700/90' : 'text-amber-700/90'"
                    data-testid="key-cooldown-reset"
                  >
                    {{ cooldownResetHint(k) }}
                  </span>
                </UiTooltip>
              </div>

              <div class="flex items-center gap-3 shrink-0">
                <!-- 行内单胶囊进度条 (Gemini 的 5h 与周用量)，显示在刷新按钮前方 -->
                <div
                  v-if="isAntigravity"
                  class="flex items-center gap-2"
                  data-testid="antigravity-quota-container"
                >
                  <!-- 跨节点锁同步中 -->
                  <div
                    v-if="compactQuotasByKey.get(k.id)?.gemini?.isLockBusy"
                    class="text-[11px] text-sky-600 font-normal select-none"
                    title="等待另一副本完成授权续期或用量刷新"
                  >
                    跨节点锁同步中...
                  </div>

                  <!-- 未探测 / 缓存过期 / 探测失败时展示占位提示（未知不再伪装数据） -->
                  <div
                    v-else-if="!compactQuotasByKey.get(k.id)?.gemini?.h5 && k.state !== 'cooling_down'"
                    class="text-[11px] text-slate-400 font-normal select-none"
                    :title="keyTestResults[k.id] ? (keyTestResults[k.id].success === false ? `上次探测失败：${keyTestResults[k.id].message || '未知错误'}` : '配额缓存已过期，点击刷新重新探测') : undefined"
                  >
                    {{ keyTestResults[k.id] && keyTestResults[k.id].success === false ? '探测失败，点击刷新重试' : '点击上方刷新查看用量' }}
                  </div>

                  <!-- 新鲜探测结果或冷却保护状态：Gemini 胶囊双进度条 (5h / 周) -->
                  <div
                    v-else-if="compactQuotasByKey.get(k.id)?.gemini?.h5 || k.state === 'cooling_down'"
                    class="flex items-center gap-1.5 px-2 py-1 bg-white/60 hover:bg-white/90 rounded-md border border-slate-200/80 text-[11px] shadow-2xs transition-colors"
                    data-testid="quota-capsule-gemini"
                  >
                    <span class="text-[11px] font-bold text-slate-700 tracking-tight">Gemini</span>
                    <!-- Gemini 5h -->
                    <UiTooltip
                      :content="`Gemini 5小时用量剩余 ${formatQuotaPercentText(compactQuotasByKey.get(k.id)?.gemini?.h5?.fraction)} (${compactQuotasByKey.get(k.id)?.gemini?.h5?.timeUntilReset || '已就绪'})`"
                    >
                      <div class="flex items-center gap-1 cursor-default">
                        <span class="text-[10px] text-slate-500 font-mono">5h</span>
                        <div class="w-16 bg-slate-200 rounded-full h-1 overflow-hidden">
                          <div
                            class="h-full rounded-full transition-all duration-300"
                            :class="getQuotaProgressColor(compactQuotasByKey.get(k.id)?.gemini?.h5?.fraction).bar"
                            :style="{ width: quotaBarWidth(compactQuotasByKey.get(k.id)?.gemini?.h5?.fraction) }"
                          />
                        </div>
                        <span
                          class="font-mono text-[10px] font-semibold tabular-nums w-[2.5rem] text-right shrink-0"
                          :class="getQuotaProgressColor(compactQuotasByKey.get(k.id)?.gemini?.h5?.fraction).text"
                        >
                          {{ formatQuotaPercentText(compactQuotasByKey.get(k.id)?.gemini?.h5?.fraction) }}
                        </span>
                      </div>
                    </UiTooltip>

                    <!-- Gemini 周用量 (无真实周数据时显示"未下发"中性占位，不再伪装 100%) -->
                    <span class="text-slate-300">|</span>
                    <UiTooltip
                      :content="compactQuotasByKey.get(k.id)?.gemini?.weekly
                        ? `Gemini 周用量剩余 ${formatQuotaPercentText(compactQuotasByKey.get(k.id)?.gemini?.weekly?.fraction)} (${compactQuotasByKey.get(k.id)?.gemini?.weekly?.timeUntilReset})`
                        : 'Gemini 周配额上游未下发（未知）'"
                    >
                      <div class="flex items-center gap-1 cursor-default">
                        <span class="text-[10px] text-slate-500 font-mono">周</span>
                        <div class="w-16 bg-slate-200 rounded-full h-1 overflow-hidden">
                          <div
                            class="h-full rounded-full transition-all duration-300"
                            :class="getQuotaProgressColor(compactQuotasByKey.get(k.id)?.gemini?.weekly?.fraction).bar"
                            :style="{ width: quotaBarWidth(compactQuotasByKey.get(k.id)?.gemini?.weekly?.fraction) }"
                          />
                        </div>
                        <span
                          class="font-mono text-[10px] font-semibold tabular-nums w-[2.5rem] text-right shrink-0"
                          :class="getQuotaProgressColor(compactQuotasByKey.get(k.id)?.gemini?.weekly?.fraction).text"
                        >
                          {{ formatQuotaPercentText(compactQuotasByKey.get(k.id)?.gemini?.weekly?.fraction) }}
                        </span>
                      </div>
                    </UiTooltip>
                  </div>
                </div>

                <!-- 编辑优先级/权重按钮 -->
                <UiTooltip content="编辑调度优先级与权重">
                  <UiButton
                    variant="ghost"
                    size="icon"
                    :aria-label="`编辑密钥 ${k.id} 优先级与权重`"
                    :disabled="!adminWriteEnabled"
                    :data-testid="`edit-key-${k.id}`"
                    class="text-slate-400 hover:text-indigo-600 hover:bg-indigo-50"
                    @click="startEditKey(k)"
                  >
                    <Icons name="edit" size="14" />
                  </UiButton>
                </UiTooltip>

                <!-- 拨测/刷新单 Key 按钮 (支持 Antigravity 在账号后单独复测/解冻) -->
                <UiTooltip :content="isAntigravity ? '探测用量并更新状态 (若配额恢复将自动解除冷却)' : '测试密钥连通性'">
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

            <!-- 行内编辑调度参数区域 (priority & weight) -->
            <UiCollapsible :open="editingKeyId === k.id">
              <div class="mt-2 p-3 bg-slate-50/90 rounded-md border border-slate-200/70 text-xs space-y-2.5">
                <div class="flex items-center justify-between">
                  <span class="font-semibold text-slate-800 text-[13px]">修改调度参数</span>
                  <button
                    type="button"
                    class="text-slate-400 hover:text-slate-600 cursor-pointer p-0.5"
                    @click="cancelEditKey"
                  >
                    <Icons name="cross" size="13" />
                  </button>
                </div>

                <div class="grid grid-cols-2 gap-3">
                  <div>
                    <label class="block text-slate-600 mb-1 font-medium">优先级 (Priority, 数值越小越优先)</label>
                    <input
                      v-model.number="editPriority"
                      type="number"
                      min="0"
                      class="w-full bg-white border border-slate-200 rounded px-2.5 py-1 text-xs text-slate-900 focus:outline-none focus:ring-1 focus:ring-indigo-500"
                      data-testid="edit-key-priority-input"
                    />
                  </div>
                  <div>
                    <label class="block text-slate-600 mb-1 font-medium">权重 (Weight, 轮询比重)</label>
                    <input
                      v-model.number="editWeight"
                      type="number"
                      min="1"
                      class="w-full bg-white border border-slate-200 rounded px-2.5 py-1 text-xs text-slate-900 focus:outline-none focus:ring-1 focus:ring-indigo-500"
                      data-testid="edit-key-weight-input"
                    />
                  </div>
                </div>

                <div class="flex items-center justify-end gap-2 pt-1 border-t border-slate-200/50">
                  <UiButton variant="ghost" size="sm" class="text-xs px-2 py-0.5" @click="cancelEditKey">
                    取消
                  </UiButton>
                  <UiButton
                    size="sm"
                    class="text-xs px-2.5 py-0.5"
                    :disabled="editSaving || !adminWriteEnabled"
                    data-testid="save-key-edit-btn"
                    @click="saveEditKey(k.id)"
                  >
                    {{ editSaving ? '保存中...' : '保存' }}
                  </UiButton>
                </div>
              </div>
            </UiCollapsible>
          </div>
        </div>
      </div>
    </UiCollapsible>
  </div>
</template>
