<script setup lang="ts">
import { ref } from 'vue';
import type { KeyView, ProviderView, KeyTestView, CreateKeyPayload } from '../../types/admin';

const props = defineProps<{
  keys: KeyView[];
  providers: ProviderView[];
  adminWriteEnabled: boolean;
  keyTestResults: Record<string, KeyTestView>;
  testingKeyIds: Set<string>;
  batchTesting: { running: boolean; current: number; total: number };
}>();

const emit = defineEmits<{
  (e: 'create', payload: CreateKeyPayload): Promise<void>;
  (e: 'delete', id: string): Promise<void>;
  (e: 'test-single', id: string): Promise<void>;
  (e: 'test-batch'): Promise<void>;
}>();

const showDrawer = ref(false);
const submitting = ref(false);
const formError = ref<string | null>(null);

const form = ref<CreateKeyPayload>({
  id: '',
  provider: '',
  api_key: '',
  priority: 1,
  weight: 10,
});

function openDrawer() {
  form.value = {
    id: `key-${Date.now().toString().slice(-4)}`,
    provider: props.providers[0]?.name || '',
    api_key: '',
    priority: 1,
    weight: 10,
  };
  formError.value = null;
  showDrawer.value = true;
}

function closeDrawer() {
  showDrawer.value = false;
  formError.value = null;
}

async function handleSubmit() {
  const id = form.value.id.trim();
  const rawKey = form.value.api_key.trim();
  if (!id) {
    formError.value = '请输入 Key ID';
    return;
  }
  if (!form.value.provider) {
    formError.value = '请选择所属 Provider';
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
    });
    closeDrawer();
  } catch (err: unknown) {
    formError.value = err instanceof Error ? err.message : String(err);
  } finally {
    submitting.value = false;
  }
}

async function handleDelete(id: string) {
  if (!confirm(`确定删除 Key "${id}" 吗？该操作将热同步连接池，在途请求不受影响。`)) {
    return;
  }
  try {
    await emit('delete', id);
  } catch (err: unknown) {
    alert(`删除失败: ${err instanceof Error ? err.message : String(err)}`);
  }
}

function handleTestSingle(id: string) {
  void emit('test-single', id);
}

function handleTestBatch() {
  void emit('test-batch');
}
</script>

<template>
  <div class="section-container">
    <div class="section-header">
      <div>
        <h2 class="section-title">Keys 密钥池与拨测</h2>
        <p class="section-desc">管理上游上机凭证，支持脱敏展示与 3s 硬超时拨测探活</p>
      </div>

      <div class="header-actions">
        <button
          type="button"
          class="test-batch-btn"
          :disabled="batchTesting.running || keys.length === 0 || !adminWriteEnabled"
          data-testid="test-all-keys-btn"
          @click="handleTestBatch"
        >
          {{ batchTesting.running ? `拨测中 (${batchTesting.current}/${batchTesting.total})...` : '全部拨测' }}
        </button>

        <button
          type="button"
          class="add-btn"
          :disabled="!adminWriteEnabled"
          :title="!adminWriteEnabled ? '只读模式下不可新增' : ''"
          data-testid="add-key-btn"
          @click="openDrawer"
        >
          + 新建 Key
        </button>
      </div>
    </div>

    <!-- 批量拨测进度条 -->
    <div v-if="batchTesting.running" class="progress-bar-container" data-testid="batch-testing-progress">
      <div
        class="progress-bar-fill"
        :style="{ width: `${(batchTesting.current / Math.max(1, batchTesting.total)) * 100}%` }"
      ></div>
    </div>

    <!-- Keys 表格 -->
    <div class="table-wrapper">
      <table class="data-table">
        <thead>
          <tr>
            <th>Key ID</th>
            <th>Provider</th>
            <th>掩码指纹 (脱敏)</th>
            <th>优先级</th>
            <th>权重</th>
            <th>连接池状态</th>
            <th>拨测状态</th>
            <th class="actions-col">操作</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="k in keys" :key="k.id" data-testid="key-row">
            <td class="font-bold">{{ k.id }}</td>
            <td><span class="badge">{{ k.provider }}</span></td>
            <td class="code-cell">{{ k.masked_key }}</td>
            <td>{{ k.priority }}</td>
            <td>{{ k.weight }}</td>
            <td>
              <span class="state-dot" :class="`state-${k.state}`"></span>
              {{ k.state }}
            </td>
            <td>
              <!-- 拨测结果徽标 -->
              <span
                v-if="keyTestResults[k.id]"
                class="probe-badge"
                :class="keyTestResults[k.id].success ? 'probe-success' : 'probe-fail'"
                data-testid="probe-result-badge"
              >
                {{ keyTestResults[k.id].success ? `${keyTestResults[k.id].latency_ms}ms` : (keyTestResults[k.id].error_code || '异常') }}
              </span>
              <span v-else-if="testingKeyIds.has(k.id)" class="probe-loading">
                拨测中...
              </span>
              <span v-else class="text-muted">-</span>
            </td>
            <td class="actions-col">
              <button
                type="button"
                class="probe-btn"
                :disabled="testingKeyIds.has(k.id) || !adminWriteEnabled"
                data-testid="test-single-key-btn"
                @click="handleTestSingle(k.id)"
              >
                {{ testingKeyIds.has(k.id) ? '测中...' : '拨测' }}
              </button>
              <button
                type="button"
                class="del-btn"
                :disabled="!adminWriteEnabled"
                data-testid="delete-key-btn"
                @click="handleDelete(k.id)"
              >
                删除
              </button>
            </td>
          </tr>
          <tr v-if="keys.length === 0">
            <td colspan="8" class="empty-cell">暂无 Key 数据</td>
          </tr>
        </tbody>
      </table>
    </div>

    <!-- 侧边抽屉 -->
    <div v-if="showDrawer" class="drawer-backdrop">
      <div class="drawer-panel" data-testid="key-drawer">
        <div class="drawer-header">
          <h3>新建 API 密钥</h3>
          <button type="button" class="close-btn" @click="closeDrawer">×</button>
        </div>

        <form class="drawer-body" @submit.prevent="handleSubmit">
          <div v-if="formError" class="form-alert">{{ formError }}</div>

          <div class="form-group">
            <label>Key ID *</label>
            <input
              v-model="form.id"
              type="text"
              placeholder="例如: key-01"
              required
              data-testid="key-id-input"
            />
          </div>

          <div class="form-group">
            <label>所属 Provider *</label>
            <select v-model="form.provider" required data-testid="key-provider-select">
              <option v-for="p in providers" :key="p.name" :value="p.name">
                {{ p.name }}
              </option>
            </select>
          </div>

          <div class="form-group">
            <label>API Key 明文 *</label>
            <input
              v-model="form.api_key"
              type="password"
              placeholder="sk-..."
              required
              data-testid="key-secret-input"
            />
            <small class="field-hint">注意：明文仅在保存成功时展示一次，后续无法二次读取。</small>
          </div>

          <div class="form-group">
            <label>优先级 Priority (数值越小优先级越高)</label>
            <input
              v-model.number="form.priority"
              type="number"
              min="0"
              data-testid="key-priority-input"
            />
          </div>

          <div class="form-group">
            <label>轮询权重 Weight</label>
            <input
              v-model.number="form.weight"
              type="number"
              min="1"
              data-testid="key-weight-input"
            />
          </div>

          <div class="drawer-footer">
            <button type="button" class="cancel-btn" @click="closeDrawer">取消</button>
            <button
              type="submit"
              class="submit-btn"
              :disabled="submitting"
              data-testid="submit-key-btn"
            >
              {{ submitting ? '创建中...' : '确认创建' }}
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
  margin-bottom: 16px;
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

.header-actions {
  display: flex;
  gap: 12px;
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

.test-batch-btn {
  padding: 8px 16px;
  font-size: 13px;
  font-weight: 600;
  background: #f1f5f9;
  border: 1px solid #cbd5e1;
  color: #334155;
  border-radius: 6px;
  cursor: pointer;
  transition: all 0.15s;
}

.test-batch-btn:hover:not(:disabled) {
  background: #e2e8f0;
}

.add-btn:disabled, .test-batch-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.progress-bar-container {
  height: 4px;
  width: 100%;
  background: #e2e8f0;
  border-radius: 2px;
  margin-bottom: 16px;
  overflow: hidden;
}

.progress-bar-fill {
  height: 100%;
  background: #2563eb;
  transition: width 0.2s ease;
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

.badge {
  padding: 2px 8px;
  border-radius: 4px;
  font-size: 11px;
  font-weight: 600;
  background: #eff6ff;
  color: #2563eb;
}

.code-cell {
  font-family: monospace;
  font-size: 12px;
  color: #475569;
}

.state-dot {
  display: inline-block;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  margin-right: 4px;
}

.state-active { background: #10b981; }
.state-cooling_down { background: #f59e0b; }
.state-exhausted { background: #ef4444; }

.probe-badge {
  padding: 2px 8px;
  border-radius: 4px;
  font-size: 11px;
  font-weight: 600;
}

.probe-success {
  background: #ecfdf5;
  color: #059669;
}

.probe-fail {
  background: #fef2f2;
  color: #dc2626;
}

.probe-loading {
  font-size: 12px;
  color: #64748b;
}

.text-muted {
  color: #94a3b8;
}

.actions-col {
  text-align: right;
}

.probe-btn {
  padding: 4px 10px;
  font-size: 12px;
  color: #0284c7;
  background: #f0f9ff;
  border: 1px solid #bae6fd;
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

.probe-btn:disabled, .del-btn:disabled {
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

.field-hint {
  display: block;
  margin-top: 4px;
  font-size: 11px;
  color: #b45309;
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
