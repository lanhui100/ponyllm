<script setup lang="ts">
/**
 * Gateway credential management section (task-28).
 *
 * Design `.agents/notes/web-users-design.md` F1–F7. Every write is server-side
 * guarded (403 for non-admin scopes); UI hiding is convenience only. The
 * one-time plaintext is shown in a dedicated modal and cleared on close — it is
 * never written to storage.
 */
import { computed, ref } from 'vue';
import type {
  GatewayKeyView,
  IssueGatewayKeyPayload,
} from '../../types/admin';
import { useGatewayKeys } from '../../composables/useGatewayKeys';
import UiButton from '../ui/UiButton.vue';
import UiBadge from '../ui/UiBadge.vue';
import Icons from '../ui/Icons.vue';

const props = defineProps<{
  /** Whether the admin write channel is open (server-side gate echo). */
  adminWriteEnabled: boolean;
  /** Current auth compatibility mode (`legacy-only` | `dual` | `strict` | ''). */
  authCompat?: string;
}>();

const emit = defineEmits<{ (e: 'notice', message: string): void }>();

const {
  gatewayKeys,
  configVersion,
  loading,
  error,
  forbidden,
  conflictDetected,
  issuedPlaintext,
  fetchAll,
  issue,
  revoke,
  clearIssued,
  clearConflict,
} = useGatewayKeys({ autoFetch: true });

void configVersion;

const showIssue = ref(false);
const submitting = ref(false);
const formError = ref<string | null>(null);
const form = ref<IssueGatewayKeyPayload>({ id: '', scope: 'inference' });
const revokeTarget = ref<GatewayKeyView | null>(null);
const revoking = ref(false);
const copied = ref(false);

/** 403 (inference login) means "valid credential, no admin-read": show a hint. */
const canRead = computed(() => !forbidden.value);
const canWrite = computed(() => props.adminWriteEnabled && canRead.value);

const scopeBadgeClass: Record<string, string> = {
  admin: 'bg-rose-100 text-rose-700 border-rose-200',
  inference: 'bg-sky-100 text-sky-700 border-sky-200',
  readonly: 'bg-slate-100 text-slate-600 border-slate-200',
};

function statusOf(k: GatewayKeyView): { label: string; cls: string } {
  // Hard-delete semantic: deleted rows never render, so there is no '已吊销' state.
  if (k.expires_at && k.expires_at * 1000 < Date.now()) {
    return { label: '已过期', cls: 'text-amber-600' };
  }
  return { label: '生效中', cls: 'text-emerald-600' };
}

function openIssue() {
  form.value = { id: '', scope: 'inference' };
  formError.value = null;
  showIssue.value = true;
}

async function submitIssue() {
  const id = form.value.id.trim();
  if (!id) {
    formError.value = 'Key ID 不能为空';
    return;
  }
  submitting.value = true;
  formError.value = null;
  try {
    await issue({ id, scope: form.value.scope });
    showIssue.value = false;
    emit('notice', `已签发 ${form.value.scope} 凭证 ${id}`);
  } catch (err: unknown) {
    formError.value = err instanceof Error ? err.message : String(err);
  } finally {
    submitting.value = false;
  }
}

async function confirmRevoke() {
  const target = revokeTarget.value;
  if (!target) return;
  revoking.value = true;
  try {
    await revoke(target.id);
    // Hard delete: the row is gone for good — drop it from local memory too.
    gatewayKeys.value = gatewayKeys.value.filter((k) => k.id !== target.id);
    emit('notice', `凭证 ${target.id} 已删除（无残留记录）`);
    revokeTarget.value = null;
  } catch (err: unknown) {
    emit('notice', err instanceof Error ? err.message : String(err));
  } finally {
    revoking.value = false;
  }
}

async function copyPlaintext() {
  const value = issuedPlaintext.value?.api_key;
  if (!value) return;
  try {
    await navigator.clipboard.writeText(value);
    copied.value = true;
    setTimeout(() => (copied.value = false), 2000);
  } catch {
    emit('notice', '复制失败，请手动选中复制');
  }
}

function closeSecret() {
  clearIssued();
  copied.value = false;
}
</script>

<template>
  <section class="space-y-4">
    <!-- auth mode / legacy banner (F4) -->
    <div
      class="swiss-card flex flex-wrap items-center gap-2 px-4 py-3 text-[13px]"
      :class="authCompat === 'strict' ? 'border-amber-200 bg-amber-50/60' : ''"
      data-testid="credentials-auth-banner"
    >
      <Icons name="lock" size="16" class="text-slate-400" />
      <span class="text-slate-600">
        鉴权兼容模式：<span class="font-mono font-semibold text-slate-900">{{ authCompat || 'dual' }}</span>
      </span>
      <span v-if="authCompat === 'strict'" class="text-amber-700">
        strict 下旧版单 token 全部 401，请确保已签发 admin 级凭证再用
      </span>
      <span v-else class="text-slate-500">
        旧版单 token（legacy）仍可用；建议为 agent 单独签发 inference 凭证
      </span>
    </div>

    <div class="flex items-center justify-between">
      <div>
        <h3 class="text-base font-semibold text-slate-900">网关访问凭证</h3>
        <p class="text-[13px] text-slate-500 mt-0.5">
          分级 key：admin 全权 / inference 仅推理+额度 / readonly 仅只读。明文只在签发时显示一次。
        </p>
      </div>
      <div class="flex items-center gap-2">
        <UiButton
          v-if="canWrite"
          variant="secondary"
          data-testid="credentials-refresh"
          @click="fetchAll()"
        >
          刷新
        </UiButton>
        <UiButton
          v-if="canWrite"
          variant="default"
          data-testid="credentials-issue"
          @click="openIssue"
        >
          + 签发凭证
        </UiButton>
      </div>
    </div>

    <!-- inference login: no admin-read at all (F5) -->
    <div v-if="!canRead" class="text-center py-16 swiss-card" data-testid="credentials-forbidden">
      <Icons name="lock" size="36" class="text-slate-300 mx-auto mb-2.5" />
      <p class="text-base font-semibold text-slate-800">当前凭证无权查看凭证清单</p>
      <p class="text-sm text-slate-500 mt-1">
        inference 级凭证不可读取管理面（防 agent 窥视凭证）。请改用 admin 或 readonly 凭证登录。
      </p>
    </div>

    <template v-else>
      <div v-if="error" class="swiss-card px-4 py-3 text-[13px] text-rose-600" data-testid="credentials-error">
        {{ error }}
      </div>

      <div
        v-if="loading && gatewayKeys.length === 0"
        class="text-center py-16 swiss-card text-sm text-slate-500"
        data-testid="credentials-loading"
      >
        正在加载凭证…
      </div>

      <div v-else-if="gatewayKeys.length === 0" class="text-center py-16 swiss-card">
        <Icons name="key" size="36" class="text-slate-300 mx-auto mb-2.5" />
        <p class="text-base font-semibold text-slate-800">暂无分级凭证</p>
        <p class="text-sm text-slate-500 mt-1">
          点击「+ 签发凭证」为 agent 分配 inference key，避免共享管理 token
        </p>
      </div>

      <div v-else class="swiss-card overflow-hidden">
        <table class="w-full text-[13px]" data-testid="credentials-table">
          <thead>
            <tr class="text-left text-slate-500 border-b border-slate-100">
              <th class="px-4 py-2.5 font-medium">Key ID</th>
              <th class="px-4 py-2.5 font-medium">作用域</th>
              <th class="px-4 py-2.5 font-medium">前缀 / 尾 4 位</th>
              <th class="px-4 py-2.5 font-medium">状态</th>
              <th class="px-4 py-2.5 font-medium text-right">操作</th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="k in gatewayKeys"
              :key="k.id"
              class="border-b border-slate-50 last:border-0"
              :data-testid="`credentials-row-${k.id}`"
            >
              <td class="px-4 py-2.5 font-mono text-slate-900">{{ k.id }}</td>
              <td class="px-4 py-2.5">
                <UiBadge :class="scopeBadgeClass[k.scope] || scopeBadgeClass.readonly">
                  {{ k.scope }}
                </UiBadge>
              </td>
              <td class="px-4 py-2.5 font-mono text-slate-500">
                {{ k.prefix }}…{{ k.last4 }}
              </td>
              <td class="px-4 py-2.5">
                <span :class="statusOf(k).cls">{{ statusOf(k).label }}</span>
              </td>
              <td class="px-4 py-2.5 text-right">
                <button
                  v-if="canWrite"
                  type="button"
                  class="text-rose-600 hover:text-rose-700 text-[13px] cursor-pointer"
                  :data-testid="`credentials-revoke-${k.id}`"
                  @click="revokeTarget = k"
                >
                  删除
                </button>
                <span v-else class="text-slate-300 text-[13px]">—</span>
                <span v-else class="text-slate-300 text-[13px]">—</span>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </template>

    <!-- issue modal -->
    <div
      v-if="showIssue"
      class="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/30 backdrop-blur-xs p-4"
      data-testid="credentials-issue-modal"
    >
      <div class="swiss-card w-full max-w-md p-5 bg-white/95">
        <h4 class="text-base font-semibold text-slate-900 mb-3">签发分级凭证</h4>
        <label class="block text-[13px] text-slate-600 mb-1">Key ID</label>
        <input
          v-model="form.id"
          type="text"
          placeholder="e.g. agent-ci-1"
          class="w-full swiss-input mb-3 font-mono"
          data-testid="credentials-issue-id"
        />
        <label class="block text-[13px] text-slate-600 mb-1">作用域</label>
        <select
          v-model="form.scope"
          class="w-full swiss-input mb-3"
          data-testid="credentials-issue-scope"
        >
          <option value="inference">inference（agent：推理 + 额度）</option>
          <option value="readonly">readonly（只读运维）</option>
          <option value="admin">admin（全权）</option>
        </select>
        <p v-if="formError" class="text-[13px] text-rose-600 mb-2">{{ formError }}</p>
        <div class="flex justify-end gap-2 mt-2">
          <UiButton variant="secondary" @click="showIssue = false">取消</UiButton>
          <UiButton variant="default" :disabled="submitting" data-testid="credentials-issue-submit" @click="submitIssue">
            {{ submitting ? '签发中…' : '确认签发' }}
          </UiButton>
        </div>
      </div>
    </div>

    <!-- one-time plaintext modal (never persisted; cleared on close) -->
    <div
      v-if="issuedPlaintext"
      class="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/30 backdrop-blur-xs p-4"
      data-testid="credentials-secret-modal"
    >
      <div class="swiss-card w-full max-w-lg p-5 bg-white/95">
        <h4 class="text-base font-semibold text-slate-900 mb-2">凭证明文（仅显示一次）</h4>
        <p class="text-[13px] text-amber-700 mb-3">
          关闭本窗口后无法再查看；服务端只保存哈希。请立即复制并妥善保存。
        </p>
        <div class="font-mono text-[13px] break-all bg-slate-50 border border-slate-200 rounded-lg px-3 py-2 mb-3" data-testid="credentials-secret-value">
          {{ issuedPlaintext.api_key }}
        </div>
        <div class="flex justify-end gap-2">
          <UiButton variant="secondary" data-testid="credentials-secret-copy" @click="copyPlaintext">
            {{ copied ? '已复制' : '复制' }}
          </UiButton>
          <UiButton variant="default" data-testid="credentials-secret-close" @click="closeSecret">
            我已保存，关闭
          </UiButton>
        </div>
      </div>
    </div>

    <!-- revoke confirm -->
    <div
      v-if="revokeTarget"
      class="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/30 backdrop-blur-xs p-4"
      data-testid="credentials-revoke-modal"
    >
      <div class="swiss-card w-full max-w-md p-5 bg-white/95">
        <h4 class="text-base font-semibold text-slate-900 mb-2">确认删除</h4>
        <p class="text-[13px] text-slate-600 mb-4">
          凭证 <span class="font-mono font-semibold">{{ revokeTarget.id }}</span>
          （{{ revokeTarget.scope }}）将被彻底删除且不留记录，使用它的客户端会立即收到 401。此操作不可撤销。
        </p>
        <div class="flex justify-end gap-2">
          <UiButton variant="secondary" @click="revokeTarget = null">取消</UiButton>
          <UiButton variant="destructive" :disabled="revoking" data-testid="credentials-revoke-confirm" @click="confirmRevoke">
            {{ revoking ? '删除中…' : '确认删除' }}
          </UiButton>
        </div>
      </div>
    </div>

    <!-- 412 conflict -->
    <div
      v-if="conflictDetected"
      class="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/30 backdrop-blur-xs p-4"
      data-testid="credentials-conflict-modal"
    >
      <div class="swiss-card w-full max-w-md p-5 bg-white/95">
        <h4 class="text-base font-semibold text-slate-900 mb-2">配置版本冲突</h4>
        <p class="text-[13px] text-slate-600 mb-4">
          配置已被其他操作修改（412）。请刷新后重试。
        </p>
        <div class="flex justify-end gap-2">
          <UiButton variant="secondary" @click="clearConflict">关闭</UiButton>
          <UiButton variant="default" @click="clearConflict(); fetchAll()">刷新</UiButton>
        </div>
      </div>
    </div>
  </section>
</template>
