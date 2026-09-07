<script setup lang="ts">
defineProps<{
  health: 'ok' | 'down' | 'degraded' | 'unknown';
  transport: 'sse' | 'polling' | 'offline';
  isDown: boolean;
}>();

const emit = defineEmits<{
  (e: 'retry'): void;
}>();
</script>

<template>
  <div class="status-banner" :class="[health, { 'banner-down': isDown }]">
    <div class="status-indicator">
      <span class="dot" :class="health" />
      <span class="label">
        网关状态: <strong>{{ health.toUpperCase() }}</strong>
      </span>
      <span class="transport-tag" :class="transport">
        {{ transport === 'sse' ? '实时 SSE' : transport === 'polling' ? '轮询 (1.5s)' : '离线' }}
      </span>
    </div>

    <div v-if="isDown" class="down-action">
      <span class="down-msg">网关连接中断或异常</span>
      <button class="retry-btn" @click="emit('retry')">重试连接</button>
    </div>
  </div>
</template>

<style scoped>
.status-banner {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 20px;
  background: #f8fafc;
  border-radius: 8px;
  border: 1px solid #e2e8f0;
  margin-bottom: 20px;
  transition: all 0.2s ease;
}

.banner-down {
  background: #fef2f2;
  border-color: #fecaca;
}

.status-indicator {
  display: flex;
  align-items: center;
  gap: 10px;
}

.dot {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  background: #94a3b8;
}

.dot.ok {
  background: #22c55e;
  box-shadow: 0 0 8px rgba(34, 197, 94, 0.4);
}

.dot.degraded {
  background: #eab308;
}

.dot.down {
  background: #ef4444;
}

.label {
  font-size: 14px;
  color: #334155;
}

.transport-tag {
  font-size: 12px;
  padding: 2px 8px;
  border-radius: 12px;
  background: #e2e8f0;
  color: #475569;
}

.transport-tag.sse {
  background: #dcfce7;
  color: #166534;
}

.transport-tag.polling {
  background: #e0f2fe;
  color: #0369a1;
}

.transport-tag.offline {
  background: #fee2e2;
  color: #991b1b;
}

.down-action {
  display: flex;
  align-items: center;
  gap: 12px;
}

.down-msg {
  font-size: 13px;
  color: #b91c1c;
}

.retry-btn {
  padding: 6px 14px;
  background: #ef4444;
  color: white;
  border: none;
  border-radius: 6px;
  cursor: pointer;
  font-size: 13px;
  font-weight: 500;
}

.retry-btn:hover {
  background: #dc2626;
}
</style>
