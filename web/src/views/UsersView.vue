<script setup lang="ts">
// B003: admin user governance panel (JWT admin only, /api/user/admin/users).
import { onMounted, ref } from 'vue';
import UiCard from '../components/ui/UiCard.vue';
import UiButton from '../components/ui/UiButton.vue';
import UiBadge from '../components/ui/UiBadge.vue';
import Icons from '../components/ui/Icons.vue';
import NavBar from '../components/NavBar.vue';
import {
  listAdminUsers,
  createAdminUser,
  updateAdminUser,
  deleteAdminUser,
  resetAdminUserPassword,
  resetAdminUserUsage,
  type AdminUserView,
} from '../lib/userApi';
import { useToast } from '../composables/useToast';

const rows = ref<AdminUserView[]>([]);
const loading = ref(true);
const error = ref('');
const { toast } = useToast();

// Create form
const username = ref('');
const password = ref('');
const role = ref<'admin' | 'user'>('user');
const displayName = ref('');
const allowedModels = ref('');
const maxTokens = ref<number | null>(null);
const creating = ref(false);

// Reset-password form (inline per row)
const resetPasswordFor = ref<string | null>(null);
const resetPasswordValue = ref('');

async function refresh(): Promise<void> {
  loading.value = true;
  error.value = '';
  try {
    rows.value = await listAdminUsers();
  } catch (e) {
    error.value = (e as Error).message ?? '加载用户列表失败';
  } finally {
    loading.value = false;
  }
}

onMounted(() => {
  void refresh();
});

async function handleCreate(): Promise<void> {
  if (creating.value) return;
  const uname = username.value.trim();
  if (uname === '' || password.value === '') {
    error.value = '用户名与密码不能为空';
    return;
  }
  creating.value = true;
  error.value = '';
  try {
    const models = allowedModels.value
      .split(',')
      .map((s) => s.trim())
      .filter((s) => s !== '');
    await createAdminUser({
      username: uname,
      password: password.value,
      role: role.value,
      name: displayName.value.trim() || undefined,
      allowed_models: models.length > 0 ? models : undefined,
      max_tokens: maxTokens.value ?? undefined,
    });
    username.value = '';
    password.value = '';
    role.value = 'user';
    displayName.value = '';
    allowedModels.value = '';
    maxTokens.value = null;
    toast.success('用户已创建');
    await refresh();
  } catch (e) {
    error.value = (e as Error).message ?? '创建失败';
  } finally {
    creating.value = false;
  }
}

async function handleToggle(user: AdminUserView): Promise<void> {
  try {
    await updateAdminUser(user.id, { enabled: !user.enabled });
    toast.success(user.enabled ? '用户已停用' : '用户已启用');
    await refresh();
  } catch (e) {
    error.value = (e as Error).message ?? '操作失败';
  }
}

async function handleRoleToggle(user: AdminUserView): Promise<void> {
  try {
    await updateAdminUser(user.id, { role: user.role === 'admin' ? 'user' : 'admin' });
    toast.success('角色已更新');
    await refresh();
  } catch (e) {
    error.value = (e as Error).message ?? '角色更新失败';
  }
}

async function handleResetUsage(user: AdminUserView): Promise<void> {
  try {
    await resetAdminUserUsage(user.id);
    toast.success('用量已重置');
    await refresh();
  } catch (e) {
    error.value = (e as Error).message ?? '用量重置失败';
  }
}

async function handleDelete(user: AdminUserView): Promise<void> {
  try {
    await deleteAdminUser(user.id);
    toast.success('用户已删除');
    await refresh();
  } catch (e) {
    error.value = (e as Error).message ?? '删除失败';
  }
}

function beginResetPassword(user: AdminUserView): void {
  resetPasswordFor.value = user.id;
  resetPasswordValue.value = '';
}

async function confirmResetPassword(): Promise<void> {
  const uid = resetPasswordFor.value;
  if (!uid) return;
  if (resetPasswordValue.value === '') {
    error.value = '新密码不能为空';
    return;
  }
  try {
    await resetAdminUserPassword(uid, resetPasswordValue.value);
    toast.success('密码已重置（旧 JWT 全部失效）');
    resetPasswordFor.value = null;
    resetPasswordValue.value = '';
  } catch (e) {
    error.value = (e as Error).message ?? '密码重置失败';
  }
}

function fmtTokens(n: number | null): string {
  return n === null ? '∞' : String(n);
}

function fmtModels(list: string[] | null): string {
  return list && list.length > 0 ? list.join(', ') : '全部模型';
}
</script>

<template>
  <div class="min-h-screen">
    <NavBar />
    <main class="max-w-5xl mx-auto px-4 sm:px-8 py-8 space-y-6">
      <div>
        <h1 class="text-xl font-bold tracking-tight text-slate-900">用户管理</h1>
        <p class="mt-1 text-xs text-slate-500">管理员专用：创建/停用/删除用户、设置角色、重置密码与用量。</p>
      </div>

      <div v-if="error" class="p-2.5 rounded-lg bg-rose-50 border border-rose-200/60 text-xs text-rose-600">
        <span class="error">{{ error }}</span>
      </div>

      <!-- Create form -->
      <UiCard class="p-5">
        <h2 class="text-sm font-semibold text-slate-800 mb-3">创建用户</h2>
        <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <div>
            <label for="user-username" class="block text-xs font-medium text-slate-700 mb-1.5">用户名（唯一）</label>
            <input
              id="user-username"
              v-model="username"
              type="text"
              placeholder="例如 alice"
              class="w-full h-10 px-3 py-2 text-sm bg-slate-50/50 border border-slate-200 rounded-lg focus:outline-none focus:bg-white focus:border-slate-400 text-slate-900 placeholder:text-slate-400 transition-all"
            />
          </div>
          <div>
            <label for="user-password" class="block text-xs font-medium text-slate-700 mb-1.5">初始密码</label>
            <input
              id="user-password"
              v-model="password"
              type="password"
              autocomplete="new-password"
              placeholder="至少 1 字符"
              class="w-full h-10 px-3 py-2 text-sm bg-slate-50/50 border border-slate-200 rounded-lg focus:outline-none focus:bg-white focus:border-slate-400 text-slate-900 placeholder:text-slate-400 transition-all"
            />
          </div>
          <div>
            <label for="user-display-name" class="block text-xs font-medium text-slate-700 mb-1.5">显示名（可选）</label>
            <input
              id="user-display-name"
              v-model="displayName"
              type="text"
              placeholder="Alice"
              class="w-full h-10 px-3 py-2 text-sm bg-slate-50/50 border border-slate-200 rounded-lg focus:outline-none focus:bg-white focus:border-slate-400 text-slate-900 placeholder:text-slate-400 transition-all"
            />
          </div>
          <div>
            <label for="user-role" class="block text-xs font-medium text-slate-700 mb-1.5">角色</label>
            <select
              id="user-role"
              v-model="role"
              class="w-full h-10 px-3 py-2 text-sm bg-slate-50/50 border border-slate-200 rounded-lg focus:outline-none focus:bg-white focus:border-slate-400 text-slate-900 transition-all"
            >
              <option value="user">user（普通用户）</option>
              <option value="admin">admin（管理员）</option>
            </select>
          </div>
          <div>
            <label for="user-models" class="block text-xs font-medium text-slate-700 mb-1.5">
              模型白名单（逗号分隔，留空=全部）
            </label>
            <input
              id="user-models"
              v-model="allowedModels"
              type="text"
              placeholder="gpt-4o-mini, deepseek/*"
              class="w-full h-10 px-3 py-2 text-sm bg-slate-50/50 border border-slate-200 rounded-lg focus:outline-none focus:bg-white focus:border-slate-400 text-slate-900 placeholder:text-slate-400 transition-all font-mono tracking-wide"
            />
          </div>
          <div>
            <label for="user-max-tokens" class="block text-xs font-medium text-slate-700 mb-1.5">用户级用量上限（留空=不限）</label>
            <input
              id="user-max-tokens"
              v-model.number="maxTokens"
              type="number"
              min="0"
              placeholder="1000000"
              class="w-full h-10 px-3 py-2 text-sm bg-slate-50/50 border border-slate-200 rounded-lg focus:outline-none focus:bg-white focus:border-slate-400 text-slate-900 placeholder:text-slate-400 transition-all"
            />
          </div>
        </div>
        <div class="mt-4 flex justify-end">
          <UiButton :disabled="creating" @click="handleCreate">
            <Icons v-if="creating" name="refresh" size="14" class="animate-spin" />
            <Icons v-else name="plus" size="14" />
            <span>{{ creating ? '创建中...' : '创建用户' }}</span>
          </UiButton>
        </div>
      </UiCard>

      <!-- Reset password inline -->
      <UiCard v-if="resetPasswordFor" class="p-4 border-amber-200/70 bg-amber-50/50">
        <div class="flex items-center gap-3">
          <input
            v-model="resetPasswordValue"
            type="password"
            placeholder="新密码"
            class="flex-1 h-10 px-3 py-2 text-sm bg-white border border-slate-200 rounded-lg focus:outline-none focus:border-slate-400 text-slate-900 placeholder:text-slate-400 transition-all"
          />
          <UiButton variant="default" @click="confirmResetPassword">
            <span>确认重置</span>
          </UiButton>
          <UiButton variant="ghost" @click="resetPasswordFor = null">
            <span>取消</span>
          </UiButton>
        </div>
      </UiCard>

      <!-- User list -->
      <UiCard class="p-5">
        <h2 class="text-sm font-semibold text-slate-800 mb-3">用户列表</h2>
        <div v-if="loading" class="text-xs text-slate-400 py-4">加载中...</div>
        <div v-else-if="rows.length === 0" class="text-xs text-slate-400 py-4">还没有用户。</div>
        <div v-else class="space-y-2.5">
          <div
            v-for="user in rows"
            :key="user.id"
            class="flex items-center justify-between gap-3 p-3 rounded-lg border border-slate-200/70 bg-slate-50/40"
          >
            <div class="min-w-0 space-y-1">
              <div class="flex items-center gap-2">
                <span class="text-sm font-medium text-slate-800 truncate">{{ user.username ?? user.id }}</span>
                <UiBadge :variant="user.enabled ? 'success' : 'secondary'">
                  {{ user.enabled ? '启用' : '停用' }}
                </UiBadge>
                <UiBadge :variant="user.role === 'admin' ? 'warning' : 'default'">
                  {{ user.role }}
                </UiBadge>
              </div>
              <div class="text-[11px] text-slate-500 font-mono truncate">{{ user.id }}</div>
              <div class="text-[11px] text-slate-500">
                模型：{{ fmtModels(user.allowed_models) }} · 用量 {{ user.used_tokens }} / {{ fmtTokens(user.max_tokens) }}
              </div>
            </div>
            <div class="flex items-center gap-1.5 shrink-0 flex-wrap">
              <UiButton variant="secondary" size="sm" @click="handleToggle(user)">
                {{ user.enabled ? '停用' : '启用' }}
              </UiButton>
              <UiButton variant="outline" size="sm" @click="handleRoleToggle(user)">
                <span>改角色</span>
              </UiButton>
              <UiButton variant="outline" size="sm" @click="beginResetPassword(user)">
                <span>重置密码</span>
              </UiButton>
              <UiButton variant="outline" size="sm" @click="handleResetUsage(user)">
                <span>重置用量</span>
              </UiButton>
              <UiButton variant="destructive" size="sm" @click="handleDelete(user)">
                <span>删除</span>
              </UiButton>
            </div>
          </div>
        </div>
      </UiCard>
    </main>
  </div>
</template>
