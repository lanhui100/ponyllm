<script setup lang="ts">
import { ref } from 'vue';
import type { ModelView, ProviderView, CreateModelPayload, UpdateModelPayload } from '../../types/admin';

const props = defineProps<{
  models: ModelView[];
  providers: ProviderView[];
  adminWriteEnabled: boolean;
}>();

const emit = defineEmits<{
  (e: 'create', payload: CreateModelPayload): Promise<void>;
  (e: 'update', name: string, payload: UpdateModelPayload): Promise<void>;
  (e: 'delete', name: string): Promise<void>;
}>();

const THINKING_TIERS = ['Off', 'Low', 'Medium', 'High'] as const;
const MODEL_TIERS = ['Fast', 'Smart', 'Large', 'Fallback'] as const;

const showDrawer = ref(false);
const isEditing = ref(false);
const editingModelName = ref('');
const submitting = ref(false);
const formError = ref<string | null>(null);

const form = ref({
  name: '',
  provider: '',
  tier: 'Smart',
  context_window: '128k',
  thinking_default: 'Off',
  thinking_max: 'High',
  protocol: '',
});

function openCreateDrawer() {
  isEditing.value = false;
  editingModelName.value = '';
  form.value = {
    name: '',
    provider: props.providers[0]?.name || '',
    tier: 'Smart',
    context_window: '128k',
    thinking_default: 'Off',
    thinking_max: 'High',
    protocol: '',
  };
  formError.value = null;
  showDrawer.value = true;
}

function openEditDrawer(model: ModelView) {
  isEditing.value = true;
  editingModelName.value = model.name;
  form.value = {
    name: model.name,
    provider: props.providers[0]?.name || '',
    tier: model.tier || 'Smart',
    context_window: model.context_window || '128k',
    thinking_default: model.thinking_default || 'Off',
    thinking_max: model.thinking_max || 'High',
    protocol: model.protocol || '',
  };
  formError.value = null;
  showDrawer.value = true;
}

function closeDrawer() {
  showDrawer.value = false;
  formError.value = null;
}

async function handleSubmit() {
  const name = form.value.name.trim();
  if (!name) {
    formError.value = '请输入模型名称';
    return;
  }
  if (!form.value.provider) {
    formError.value = '请选择所属 Provider';
    return;
  }

  submitting.value = true;
  formError.value = null;
  try {
    if (isEditing.value) {
      await emit('update', editingModelName.value, {
        provider: form.value.provider,
        tier: form.value.tier,
        context_window: form.value.context_window,
        thinking_default: form.value.thinking_default,
        thinking_max: form.value.thinking_max,
        protocol: form.value.protocol || null,
      });
    } else {
      await emit('create', {
        name,
        provider: form.value.provider,
        tier: form.value.tier,
        context_window: form.value.context_window,
        thinking_default: form.value.thinking_default,
        thinking_max: form.value.thinking_max,
        protocol: form.value.protocol || null,
      });
    }
    closeDrawer();
  } catch (err: unknown) {
    formError.value = err instanceof Error ? err.message : String(err);
  } finally {
    submitting.value = false;
  }
}

async function handleDelete(name: string) {
  if (!confirm(`确定删除模型 "${name}" 吗？该操作不会中断当前正在处理的在途请求。`)) {
    return;
  }
  try {
    await emit('delete', name);
  } catch (err: unknown) {
    alert(`删除失败: ${err instanceof Error ? err.message : String(err)}`);
  }
}
</script>

<template>
  <div class="section-container">
    <div class="section-header">
      <div>
        <h2 class="section-title">Models 模型路由字典</h2>
        <p class="section-desc">维护可用模型字典、分级 Tier 及思考强度 (Thinking) 地板与天花板</p>
      </div>
      <button
        type="button"
        class="add-btn"
        :disabled="!adminWriteEnabled"
        :title="!adminWriteEnabled ? '只读模式下不可新增' : ''"
        data-testid="add-model-btn"
        @click="openCreateDrawer"
      >
        + 新建 Model
      </button>
    </div>

    <!-- Model 表格 -->
    <div class="table-wrapper">
      <table class="data-table">
        <thead>
          <tr>
            <th>模型名称</th>
            <th>分级 Tier</th>
            <th>上下文窗口</th>
            <th>思考默认 (地板)</th>
            <th>思考上限 (天花板)</th>
            <th>协议</th>
            <th class="actions-col">操作</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="m in models" :key="m.name" data-testid="model-row">
            <td class="font-bold">{{ m.name }}</td>
            <td>
              <span class="tier-tag" :class="`tier-${m.tier?.toLowerCase()}`">
                {{ m.tier }}
              </span>
            </td>
            <td>{{ m.context_window }}</td>
            <td><span class="thinking-badge">{{ m.thinking_default }}</span></td>
            <td><span class="thinking-badge">{{ m.thinking_max }}</span></td>
            <td>{{ m.protocol || '自动推断' }}</td>
            <td class="actions-col">
              <button
                type="button"
                class="edit-btn"
                :disabled="!adminWriteEnabled"
                data-testid="edit-model-btn"
                @click="openEditDrawer(m)"
              >
                编辑
              </button>
              <button
                type="button"
                class="del-btn"
                :disabled="!adminWriteEnabled"
                data-testid="delete-model-btn"
                @click="handleDelete(m.name)"
              >
                删除
              </button>
            </td>
          </tr>
          <tr v-if="models.length === 0">
            <td colspan="7" class="empty-cell">暂无 Model 数据</td>
          </tr>
        </tbody>
      </table>
    </div>

    <!-- 侧边抽屉 -->
    <div v-if="showDrawer" class="drawer-backdrop">
      <div class="drawer-panel" data-testid="model-drawer">
        <div class="drawer-header">
          <h3>{{ isEditing ? '编辑 Model 配置' : '新建 Model' }}</h3>
          <button type="button" class="close-btn" @click="closeDrawer">×</button>
        </div>

        <form class="drawer-body" @submit.prevent="handleSubmit">
          <div v-if="formError" class="form-alert">{{ formError }}</div>

          <div class="form-group">
            <label>模型名称 *</label>
            <input
              v-model="form.name"
              type="text"
              placeholder="例如: gpt-4o / claude-3-5-sonnet"
              :disabled="isEditing"
              required
              data-testid="model-name-input"
            />
          </div>

          <div class="form-group">
            <label>所属 Provider *</label>
            <select v-model="form.provider" required data-testid="model-provider-select">
              <option v-for="p in providers" :key="p.name" :value="p.name">
                {{ p.name }}
              </option>
            </select>
          </div>

          <div class="form-group">
            <label>分级 Tier</label>
            <select v-model="form.tier" data-testid="model-tier-select">
              <option v-for="t in MODEL_TIERS" :key="t" :value="t">{{ t }}</option>
            </select>
          </div>

          <div class="form-group">
            <label>上下文窗口</label>
            <input
              v-model="form.context_window"
              type="text"
              placeholder="例如: 128k / 200k"
              data-testid="model-context-window-input"
            />
          </div>

          <div class="form-group">
            <label>思考强度默认值 (地板)</label>
            <select v-model="form.thinking_default" data-testid="thinking-default-select">
              <option v-for="th in THINKING_TIERS" :key="th" :value="th">{{ th }}</option>
            </select>
          </div>

          <div class="form-group">
            <label>思考强度上限 (天花板)</label>
            <select v-model="form.thinking_max" data-testid="thinking-max-select">
              <option v-for="th in THINKING_TIERS" :key="th" :value="th">{{ th }}</option>
            </select>
          </div>

          <div class="drawer-footer">
            <button type="button" class="cancel-btn" @click="closeDrawer">取消</button>
            <button
              type="submit"
              class="submit-btn"
              :disabled="submitting"
              data-testid="submit-model-btn"
            >
              {{ submitting ? '保存中...' : '确认保存' }}
            </button>
          </div>
        </form>
      </div>
    </div>
  </div>
</template>

<style scoped>
.section-container {
  background: #ffffff;
  border: 1px solid #e2e8f0;
  border-radius: 8px;
  padding: 20px;
}

.section-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 20px;
}

.section-title {
  font-size: 18px;
  font-weight: 700;
  color: #0f172a;
  margin: 0 0 4px 0;
}

.section-desc {
  font-size: 13px;
  color: #64748b;
  margin: 0;
}

.add-btn {
  padding: 8px 16px;
  font-size: 13px;
  font-weight: 600;
  background: #2563eb;
  color: #ffffff;
  border: none;
  border-radius: 6px;
  cursor: pointer;
}

.add-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.table-wrapper {
  overflow-x: auto;
}

.data-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 13px;
  text-align: left;
}

.data-table th {
  background: #f8fafc;
  padding: 10px 14px;
  font-weight: 600;
  color: #475569;
  border-bottom: 1px solid #e2e8f0;
}

.data-table td {
  padding: 12px 14px;
  border-bottom: 1px solid #f1f5f9;
  color: #334155;
}

.font-bold {
  font-weight: 600;
  color: #0f172a;
}

.tier-tag {
  display: inline-block;
  padding: 2px 8px;
  border-radius: 4px;
  font-size: 11px;
  font-weight: 600;
  background: #f1f5f9;
  color: #475569;
}

.tier-fast { background: #ecfdf5; color: #059669; }
.tier-smart { background: #eff6ff; color: #2563eb; }
.tier-large { background: #fdf4ff; color: #c026d3; }

.thinking-badge {
  padding: 2px 6px;
  border-radius: 4px;
  font-size: 11px;
  background: #f8fafc;
  border: 1px solid #e2e8f0;
  color: #475569;
}

.actions-col {
  text-align: right;
}

.edit-btn {
  padding: 4px 10px;
  font-size: 12px;
  color: #2563eb;
  background: #eff6ff;
  border: 1px solid #dbeafe;
  border-radius: 4px;
  cursor: pointer;
  margin-right: 8px;
}

.del-btn {
  padding: 4px 10px;
  font-size: 12px;
  color: #dc2626;
  background: #fef2f2;
  border: 1px solid #fee2e2;
  border-radius: 4px;
  cursor: pointer;
}

.edit-btn:disabled, .del-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.empty-cell {
  text-align: center;
  padding: 32px;
  color: #94a3b8;
}

/* 抽屉 */
.drawer-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(15, 23, 42, 0.4);
  display: flex;
  justify-content: flex-end;
  z-index: 900;
}

.drawer-panel {
  width: 100%;
  max-width: 440px;
  background: #ffffff;
  height: 100%;
  box-shadow: -4px 0 15px rgba(0, 0, 0, 0.05);
  display: flex;
  flex-direction: column;
}

.drawer-header {
  padding: 16px 20px;
  border-bottom: 1px solid #e2e8f0;
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.drawer-header h3 {
  margin: 0;
  font-size: 16px;
  font-weight: 700;
}

.close-btn {
  background: transparent;
  border: none;
  font-size: 20px;
  cursor: pointer;
  color: #64748b;
}

.drawer-body {
  padding: 20px;
  flex: 1;
  overflow-y: auto;
}

.form-alert {
  padding: 10px 12px;
  background: #fef2f2;
  border: 1px solid #fee2e2;
  border-radius: 6px;
  color: #b91c1c;
  font-size: 13px;
  margin-bottom: 16px;
}

.form-group {
  margin-bottom: 16px;
}

.form-group label {
  display: block;
  font-size: 13px;
  font-weight: 600;
  color: #334155;
  margin-bottom: 6px;
}

.form-group input, .form-group select {
  width: 100%;
  padding: 8px 12px;
  border: 1px solid #cbd5e1;
  border-radius: 6px;
  font-size: 13px;
  box-sizing: border-box;
}

.drawer-footer {
  padding: 16px 20px;
  border-top: 1px solid #e2e8f0;
  display: flex;
  justify-content: flex-end;
  gap: 12px;
}

.cancel-btn {
  padding: 8px 14px;
  background: transparent;
  border: 1px solid #cbd5e1;
  border-radius: 6px;
  font-size: 13px;
  cursor: pointer;
}

.submit-btn {
  padding: 8px 16px;
  background: #2563eb;
  color: #ffffff;
  border: none;
  border-radius: 6px;
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
}
</style>
