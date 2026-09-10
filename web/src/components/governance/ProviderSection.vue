<script setup lang="ts">
import { ref } from 'vue';
import type { ProviderView, CreateProviderPayload } from '../../types/admin';
import { toast } from '../../composables/useToast';

const props = defineProps<{
  providers: ProviderView[];
  adminWriteEnabled: boolean;
}>();

const emit = defineEmits<{
  (e: 'create', payload: CreateProviderPayload): Promise<void>;
  (e: 'delete', name: string): Promise<void>;
}>();

const showDrawer = ref(false);
const submitting = ref(false);
const formError = ref<string | null>(null);

const form = ref<CreateProviderPayload>({
  name: '',
  base_url: '',
  default_model: '',
  strategy: 'economy',
  billing_mode: 'token',
  input_price: 0,
  cached_price: 0,
  output_price: 0,
});

function openDrawer() {
  form.value = {
    name: '',
    base_url: '',
    default_model: '',
    strategy: 'economy',
    billing_mode: 'token',
    input_price: 0,
    cached_price: 0,
    output_price: 0,
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
  const url = form.value.base_url.trim();
  if (!name) {
    formError.value = '请输入 Provider 名称';
    return;
  }
  if (!url || (!url.startsWith('http://') && !url.startsWith('https://'))) {
    formError.value = 'Base URL 格式无效（必须以 http:// 或 https:// 开头）';
    return;
  }

  submitting.value = true;
  formError.value = null;
  try {
    await emit('create', {
      ...form.value,
      name,
      base_url: url,
    });
    closeDrawer();
  } catch (err: unknown) {
    formError.value = err instanceof Error ? err.message : String(err);
  } finally {
    submitting.value = false;
  }
}

async function handleDelete(name: string) {
  const confirmed = await toast.confirm({
    title: '删除 Provider',
    message: `确定删除 Provider "${name}" 吗？该操作将级联影响下属 Model 与 Key，在途请求不受影响。`,
    confirmText: '确认删除',
    cancelText: '取消',
    variant: 'destructive',
  });
  if (!confirmed) {
    return;
  }
  try {
    await emit('delete', name);
    toast.success(`Provider "${name}" 已删除`);
  } catch (err: unknown) {
    toast.error(`删除失败: ${err instanceof Error ? err.message : String(err)}`);
  }
}
</script>

<template>
  <div class="section-container">
    <div class="section-header">
      <div>
        <h2 class="section-title">Providers 模型服务商</h2>
        <p class="section-desc">管理上游 LLM 服务供应商、基地址及计费策略</p>
      </div>
      <button
        type="button"
        class="add-btn"
        :disabled="!adminWriteEnabled"
        :title="!adminWriteEnabled ? '只读模式下不可新增' : ''"
        data-testid="add-provider-btn"
        @click="openDrawer"
      >
        + 新建 Provider
      </button>
    </div>

    <!-- Provider 表格 -->
    <div class="table-wrapper">
      <table class="data-table">
        <thead>
          <tr>
            <th>Provider 名称</th>
            <th>Base URL</th>
            <th>默认模型</th>
            <th>路由策略</th>
            <th>模型数</th>
            <th>计费 ($/M tok)</th>
            <th class="actions-col">操作</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="p in providers" :key="p.name" data-testid="provider-row">
            <td class="font-bold">{{ p.name }}</td>
            <td class="code-cell">{{ p.base_url }}</td>
            <td>{{ p.default_model || '-' }}</td>
            <td><span class="badge">{{ p.strategy }}</span></td>
            <td>{{ p.models }}</td>
            <td class="price-cell">
              入: {{ p.input_price }} / 缓: {{ p.cached_price }} / 出: {{ p.output_price }}
            </td>
            <td class="actions-col">
              <button
                type="button"
                class="del-btn"
                :disabled="!adminWriteEnabled"
                data-testid="delete-provider-btn"
                @click="handleDelete(p.name)"
              >
                删除
              </button>
            </td>
          </tr>
          <tr v-if="providers.length === 0">
            <td colspan="7" class="empty-cell">暂无 Provider 数据</td>
          </tr>
        </tbody>
      </table>
    </div>

    <!-- 侧边抽屉 / 弹窗表单 -->
    <div v-if="showDrawer" class="drawer-backdrop">
      <div class="drawer-panel" data-testid="provider-drawer">
        <div class="drawer-header">
          <h3>新建 Provider</h3>
          <button type="button" class="close-btn" @click="closeDrawer">×</button>
        </div>

        <form class="drawer-body" @submit.prevent="handleSubmit">
          <div v-if="formError" class="form-alert">{{ formError }}</div>

          <div class="form-group">
            <label>Provider 标识 *</label>
            <input
              v-model="form.name"
              type="text"
              placeholder="例如: openai / deepseek"
              required
              data-testid="provider-name-input"
            />
          </div>

          <div class="form-group">
            <label>Base URL *</label>
            <input
              v-model="form.base_url"
              type="url"
              placeholder="https://api.openai.com/v1"
              required
              data-testid="provider-base-url-input"
            />
          </div>

          <div class="form-group">
            <label>默认模型</label>
            <input
              v-model="form.default_model"
              type="text"
              placeholder="例如: gpt-4o"
              data-testid="provider-default-model-input"
            />
          </div>

          <div class="form-group">
            <label>计费单价 ($/M tokens)</label>
            <div class="price-inputs">
              <input v-model.number="form.input_price" type="number" step="0.01" placeholder="输入" />
              <input v-model.number="form.cached_price" type="number" step="0.01" placeholder="缓存" />
              <input v-model.number="form.output_price" type="number" step="0.01" placeholder="输出" />
            </div>
          </div>

          <div class="drawer-footer">
            <button type="button" class="cancel-btn" @click="closeDrawer">取消</button>
            <button
              type="submit"
              class="submit-btn"
              :disabled="submitting"
              data-testid="submit-provider-btn"
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
  transition: opacity 0.15s;
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

.code-cell {
  font-family: var(--font-mono-family, monospace);
  font-size: 12px;
  color: #2563eb;
}

.price-cell {
  font-size: 12px;
  color: #64748b;
}

.badge {
  padding: 2px 8px;
  border-radius: 4px;
  font-size: 11px;
  font-weight: 600;
  background: #eff6ff;
  color: #2563eb;
  text-transform: capitalize;
}

.actions-col {
  text-align: right;
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

.del-btn:disabled {
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

.form-group input {
  width: 100%;
  padding: 8px 12px;
  border: 1px solid #cbd5e1;
  border-radius: 6px;
  font-size: 13px;
  box-sizing: border-box;
}

.price-inputs {
  display: grid;
  grid-template-columns: 1fr 1fr 1fr;
  gap: 8px;
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
