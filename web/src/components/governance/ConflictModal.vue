<script setup lang="ts">
defineProps<{
  show: boolean;
}>();

const emit = defineEmits<{
  (e: 'refresh'): void;
  (e: 'close'): void;
}>();
</script>

<template>
  <div v-if="show" class="modal-backdrop">
    <div class="modal-card">
      <div class="modal-header">
        <h3 class="modal-title">配置版本冲突 (HTTP 412)</h3>
      </div>

      <div class="modal-body">
        <div class="alert-conflict">
          <strong>并发修改拦截：</strong>
          当前网关配置已被其他管理员或外部进程修改（版本号不匹配）。为了防止误覆盖他人变更，您的提交已被安全拦截。
        </div>

        <p class="modal-text">
          请点击下方“拉取最新配置”获取远端最新状态，核对差异后再次尝试提交。
        </p>
      </div>

      <div class="modal-footer">
        <button
          type="button"
          class="cancel-btn"
          data-testid="close-conflict-modal-btn"
          @click="emit('close')"
        >
          暂不刷新
        </button>
        <button
          type="button"
          class="refresh-btn"
          data-testid="refresh-config-btn"
          @click="emit('refresh')"
        >
          拉取最新配置
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
  max-width: 500px;
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
  color: #dc2626;
}

.modal-body {
  padding: 20px;
}

.alert-conflict {
  background: #fef2f2;
  border: 1px solid #fee2e2;
  color: #b91c1c;
  font-size: 13px;
  line-height: 1.5;
  padding: 12px;
  border-radius: 8px;
  margin-bottom: 16px;
}

.modal-text {
  font-size: 14px;
  color: #475569;
  line-height: 1.6;
  margin: 0;
}

.modal-footer {
  padding: 16px 20px;
  border-top: 1px solid #e2e8f0;
  display: flex;
  justify-content: flex-end;
  gap: 12px;
  background: #f8fafc;
}

.cancel-btn {
  padding: 8px 14px;
  font-size: 13px;
  font-weight: 500;
  background: transparent;
  border: 1px solid #cbd5e1;
  border-radius: 6px;
  color: #64748b;
  cursor: pointer;
  transition: all 0.15s;
}

.cancel-btn:hover {
  background: #f1f5f9;
  color: #334155;
}

.refresh-btn {
  padding: 8px 18px;
  font-size: 13px;
  font-weight: 600;
  background: #dc2626;
  color: #ffffff;
  border: none;
  border-radius: 6px;
  cursor: pointer;
  transition: background 0.15s;
}

.refresh-btn:hover {
  background: #b91c1c;
}
</style>
