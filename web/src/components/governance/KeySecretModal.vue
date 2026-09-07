<script setup lang="ts">
import { ref } from 'vue';
import type { CreateKeyResponse } from '../../types/admin';

const props = defineProps<{
  keyResult: CreateKeyResponse | null;
}>();

const emit = defineEmits<{
  (e: 'close'): void;
}>();

const copied = ref(false);

async function handleCopy() {
  if (!props.keyResult?.api_key) return;
  try {
    await navigator.clipboard.writeText(props.keyResult.api_key);
    copied.value = true;
    setTimeout(() => {
      copied.value = false;
    }, 2000);
  } catch {
    // Clipboard API might fail in non-secure or test environments
  }
}

function handleClose() {
  emit('close');
}
</script>

<template>
  <div v-if="keyResult" class="modal-backdrop">
    <div class="modal-card">
      <div class="modal-header">
        <h3 class="modal-title">API 密钥创建成功</h3>
      </div>

      <div class="modal-body">
        <div class="alert-warning">
          <strong>安全警示：</strong>
          该密钥明文仅在创建瞬间显示一次。网关后续仅返回脱敏掩码，且绝不持久化在浏览器端。请立即复制妥善保存！
        </div>

        <div class="key-display-box">
          <label class="box-label">密钥明文 (ID: {{ keyResult.id }})</label>
          <div class="key-value-row">
            <input
              type="text"
              readonly
              :value="keyResult.api_key"
              class="key-input"
              data-testid="plaintext-key-input"
            />
            <button
              type="button"
              class="copy-btn"
              data-testid="copy-key-btn"
              @click="handleCopy"
            >
              {{ copied ? '已复制' : '复制密钥' }}
            </button>
          </div>
        </div>

        <div class="meta-row">
          <span>所属 Provider: <strong>{{ keyResult.provider }}</strong></span>
          <span>权重: {{ keyResult.weight }}</span>
          <span>优先级: {{ keyResult.priority }}</span>
        </div>
      </div>

      <div class="modal-footer">
        <button
          type="button"
          class="confirm-btn"
          data-testid="close-key-modal-btn"
          @click="handleClose"
        >
          我已安全记录，关闭
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.modal-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(15, 23, 42, 0.6);
  backdrop-filter: blur(2px);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 999;
}

.modal-card {
  background: #ffffff;
  border-radius: 12px;
  width: 100%;
  max-width: 520px;
  box-shadow: 0 20px 25px -5px rgba(0, 0, 0, 0.1), 0 10px 10px -5px rgba(0, 0, 0, 0.04);
  overflow: hidden;
}

.modal-header {
  padding: 16px 20px;
  border-bottom: 1px solid #e2e8f0;
}

.modal-title {
  margin: 0;
  font-size: 16px;
  font-weight: 700;
  color: #0f172a;
}

.modal-body {
  padding: 20px;
}

.alert-warning {
  background: #fffbeb;
  border: 1px solid #fef3c7;
  color: #b45309;
  font-size: 13px;
  line-height: 1.5;
  padding: 12px;
  border-radius: 8px;
  margin-bottom: 16px;
}

.key-display-box {
  margin-bottom: 16px;
}

.box-label {
  display: block;
  font-size: 12px;
  font-weight: 600;
  color: #475569;
  margin-bottom: 6px;
}

.key-value-row {
  display: flex;
  gap: 8px;
}

.key-input {
  flex: 1;
  font-family: monospace;
  font-size: 13px;
  background: #f8fafc;
  border: 1px solid #cbd5e1;
  border-radius: 6px;
  padding: 8px 12px;
  color: #0f172a;
}

.copy-btn {
  padding: 8px 14px;
  font-size: 13px;
  font-weight: 600;
  background: #f1f5f9;
  border: 1px solid #cbd5e1;
  border-radius: 6px;
  color: #334155;
  cursor: pointer;
  white-space: nowrap;
  transition: all 0.15s;
}

.copy-btn:hover {
  background: #e2e8f0;
}

.meta-row {
  display: flex;
  gap: 16px;
  font-size: 13px;
  color: #64748b;
}

.modal-footer {
  padding: 16px 20px;
  border-top: 1px solid #e2e8f0;
  display: flex;
  justify-content: flex-end;
  background: #f8fafc;
}

.confirm-btn {
  padding: 8px 18px;
  font-size: 14px;
  font-weight: 600;
  background: #2563eb;
  color: #ffffff;
  border: none;
  border-radius: 6px;
  cursor: pointer;
  transition: background 0.15s;
}

.confirm-btn:hover {
  background: #1d4ed8;
}
</style>
