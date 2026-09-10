<script setup lang="ts">
import { ref, computed, watch, onMounted, onUnmounted } from 'vue';
import type { RecordedFrame } from '../types/telemetry';
import { scrubSecrets, maskKey, generateCurlCommand } from '../utils/scrub';

const props = defineProps<{
  frame: RecordedFrame | null;
  isOpen: boolean;
}>();

const emit = defineEmits<{
  (e: 'close'): void;
}>();

const copied = ref(false);

const maskedKey = computed(() => {
  return maskKey(props.frame?.sanitized_key || props.frame?.key_id || '');
});

const scrubbedError = computed(() => {
  return props.frame?.error ? scrubSecrets(props.frame.error) : '';
});

const scrubbedRequest = computed(() => {
  return props.frame?.request_snippet ? scrubSecrets(props.frame.request_snippet) : '';
});

const scrubbedResponse = computed(() => {
  return props.frame?.response_snippet ? scrubSecrets(props.frame.response_snippet) : '';
});

const curlCommand = computed(() => {
  if (!props.frame) return '';
  const origin = typeof window !== 'undefined' ? window.location.origin : '';
  return generateCurlCommand(props.frame, origin);
});

async function copyCurl() {
  if (!curlCommand.value) return;
  try {
    await navigator.clipboard.writeText(curlCommand.value);
    copied.value = true;
    setTimeout(() => {
      copied.value = false;
    }, 2000);
  } catch {
    // Fallback or permission denied
  }
}

function handleKeyDown(e: KeyboardEvent) {
  if (e.key === 'Escape' && props.isOpen) {
    emit('close');
  }
}

onMounted(() => {
  if (typeof window !== 'undefined') {
    window.addEventListener('keydown', handleKeyDown);
  }
});

onUnmounted(() => {
  if (typeof window !== 'undefined') {
    window.removeEventListener('keydown', handleKeyDown);
    if (typeof document !== 'undefined') {
      document.body.style.overflow = '';
    }
  }
});

watch(
  () => props.isOpen,
  (open) => {
    if (typeof document !== 'undefined') {
      document.body.style.overflow = open ? 'hidden' : '';
    }
  },
  { immediate: true }
);
</script>

<template>
  <div v-if="isOpen && frame" class="drawer-backdrop" @click="emit('close')">
    <div
      class="drawer-panel"
      role="dialog"
      aria-modal="true"
      aria-labelledby="drawer-title"
      @click.stop
    >
      <div class="drawer-header">
        <div>
          <h2 id="drawer-title" class="drawer-title">录波帧详情</h2>
          <span class="req-id">{{ frame.request_id }}</span>
        </div>
        <button class="close-btn" aria-label="关闭录波详情" @click="emit('close')">×</button>
      </div>

      <div class="drawer-body">
        <div class="section meta-grid">
          <div class="meta-item">
            <span class="meta-label">状态码</span>
            <span class="meta-val badge" :class="(frame.status_code ?? 0) < 400 ? 'status-ok' : 'status-err'">
              {{ frame.status_code ?? '--' }}
            </span>
          </div>

          <div class="meta-item">
            <span class="meta-label">耗时</span>
            <span class="meta-val">{{ frame.latency_ms }} ms</span>
          </div>

          <div class="meta-item">
            <span class="meta-label">端点</span>
            <span class="meta-val font-mono">{{ frame.endpoint }}</span>
          </div>

          <div class="meta-item">
            <span class="meta-label">Provider</span>
            <span class="meta-val">{{ frame.provider || '--' }}</span>
          </div>

          <div class="meta-item">
            <span class="meta-label">Key 标识</span>
            <span class="meta-val font-mono">{{ frame.key_id }}</span>
          </div>

          <div class="meta-item">
            <span class="meta-label">脱敏 Key</span>
            <span class="meta-val font-mono text-masked">{{ maskedKey }}</span>
          </div>

          <div class="meta-item">
            <span class="meta-label">时间戳</span>
            <span class="meta-val">{{ new Date(frame.timestamp).toLocaleString() }}</span>
          </div>
        </div>

        <div v-if="scrubbedError" class="section error-box">
          <div class="section-heading text-err">错误异常</div>
          <pre class="code-block err-content">{{ scrubbedError }}</pre>
        </div>

        <div v-if="frame.stream_flow" class="section">
          <div class="section-heading">流传输指标 (Stream Flow)</div>
          <div class="stream-grid">
            <div class="stream-item">
              <span class="label">TTFT</span>
              <span class="val">{{ frame.stream_flow.ttft_ms !== undefined ? `${Math.round(frame.stream_flow.ttft_ms)} ms` : '--' }}</span>
            </div>
            <div class="stream-item">
              <span class="label">TTLB</span>
              <span class="val">{{ frame.stream_flow.ttlb_ms !== undefined ? `${Math.round(frame.stream_flow.ttlb_ms)} ms` : '--' }}</span>
            </div>
            <div class="stream-item">
              <span class="label">Chunks</span>
              <span class="val">{{ frame.stream_flow.chunks ?? '--' }}</span>
            </div>
            <div class="stream-item">
              <span class="label">Bytes</span>
              <span class="val">{{ frame.stream_flow.bytes ?? '--' }}</span>
            </div>
            <div class="stream-item">
              <span class="label">TPS</span>
              <span class="val">{{ frame.stream_flow.tps !== undefined ? `${Math.round(frame.stream_flow.tps)} tok/s` : '--' }}</span>
            </div>
            <div class="stream-item">
              <span class="label">Stalls</span>
              <span class="val">{{ frame.stream_flow.stall_count ?? 0 }}</span>
            </div>
          </div>
        </div>

        <div v-if="scrubbedRequest" class="section">
          <div class="section-heading">请求载荷 (脱敏)</div>
          <pre class="code-block">{{ scrubbedRequest }}</pre>
        </div>

        <div v-if="scrubbedResponse" class="section">
          <div class="section-heading">响应摘要 (脱敏)</div>
          <pre class="code-block">{{ scrubbedResponse }}</pre>
        </div>

        <div class="section">
          <div class="section-header-row">
            <div class="section-heading">cURL 复现命令</div>
            <button class="copy-btn" @click="copyCurl">
              {{ copied ? '已复制 ✓' : '复制命令' }}
            </button>
          </div>
          <pre class="code-block curl-block">{{ curlCommand }}</pre>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.drawer-backdrop {
  position: fixed;
  top: 0;
  left: 0;
  width: 100vw;
  height: 100vh;
  background: rgba(15, 23, 42, 0.35);
  backdrop-filter: blur(6px);
  -webkit-backdrop-filter: blur(6px);
  z-index: 999;
  display: flex;
  justify-content: flex-end;
}

.drawer-panel {
  width: 580px;
  max-width: 90vw;
  height: 100vh;
  background: rgba(255, 255, 255, 0.92);
  backdrop-filter: blur(20px);
  -webkit-backdrop-filter: blur(20px);
  box-shadow: -4px 0 32px rgba(0, 0, 0, 0.08);
  display: flex;
  flex-direction: column;
}

.drawer-header {
  padding: 20px 24px;
  border-bottom: 1px solid #e2e8f0;
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
}

.drawer-title {
  margin: 0 0 4px 0;
  font-size: 19px;
  font-weight: 700;
  color: #0f172a;
}

.req-id {
  font-size: 13px;
  color: #64748b;
}

.close-btn {
  background: none;
  border: none;
  font-size: 24px;
  color: #94a3b8;
  cursor: pointer;
  line-height: 1;
}

.close-btn:hover {
  color: #0f172a;
}

.drawer-body {
  padding: 20px 24px;
  overflow-y: auto;
  flex: 1;
}

.section {
  margin-bottom: 20px;
}

.section-heading {
  font-size: 14px;
  font-weight: 600;
  color: #334155;
  margin-bottom: 8px;
}

.section-header-row {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 8px;
}

.meta-grid {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: 12px;
  background: #f8fafc;
  padding: 16px;
  border-radius: 8px;
  border: 1px solid #e2e8f0;
}

.meta-item {
  display: flex;
  flex-direction: column;
  gap: 3px;
  min-width: 0;
}

.meta-label {
  font-size: 12px;
  font-weight: 500;
  color: #64748b;
}

.meta-val {
  font-size: 14px;
  color: #0f172a;
  word-break: break-all;
  overflow-wrap: anywhere;
}

.text-masked {
  color: #0284c7;
  font-weight: 600;
}

.badge {
  display: inline-block;
  width: fit-content;
  font-size: 12px;
  padding: 2px 7px;
  border-radius: 4px;
  font-weight: 600;
}

.status-ok {
  background: #dcfce7;
  color: #166534;
  border: 1px solid #bbf7d0;
}

.status-err {
  background: #fee2e2;
  color: #991b1b;
  border: 1px solid #fecaca;
}

.stream-grid {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 10px;
  background: #f8fafc;
  padding: 12px;
  border-radius: 8px;
  border: 1px solid #e2e8f0;
}

.stream-item {
  display: flex;
  flex-direction: column;
}

.stream-item .label {
  font-size: 12px;
  font-weight: 500;
  color: #64748b;
}

.stream-item .val {
  font-size: 14px;
  font-weight: 600;
  color: #0f172a;
}

.code-block {
  background: #0f172a;
  color: #f8fafc;
  padding: 12px 14px;
  border-radius: 6px;
  font-size: 13px;
  line-height: 1.5;
  overflow-x: auto;
  white-space: pre-wrap;
  word-break: break-all;
  margin: 0;
  border: 1px solid #1e293b;
}

.err-content {
  background: #fef2f2;
  color: #991b1b;
  border: 1px solid #fecaca;
}

.copy-btn {
  padding: 4px 12px;
  font-size: 13px;
  background: #f1f5f9;
  border: 1px solid #cbd5e1;
  border-radius: 4px;
  color: #334155;
  cursor: pointer;
  font-weight: 500;
  transition: all 0.15s ease;
}

.copy-btn:hover {
  background: #e2e8f0;
  border-color: #94a3b8;
}
</style>
