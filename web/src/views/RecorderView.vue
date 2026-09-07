<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted } from 'vue';
import NavBar from '../components/NavBar.vue';
import FrameDrawer from '../components/FrameDrawer.vue';
import type { RecordedFrame } from '../types/telemetry';
import { maskKey } from '../utils/scrub';
import { useSessionStore } from '../stores/session';
import { onStopPolling } from '../router';

const frames = ref<RecordedFrame[]>([]);
const loading = ref(false);
const selectedIndex = ref<number>(-1);
const activeFrame = ref<RecordedFrame | null>(null);
const isDrawerOpen = ref(false);

const filterEndpoint = ref('');
const filterStatus = ref('all');
const filterProvider = ref('all');

const viewportRef = ref<HTMLDivElement | null>(null);
const scrollTop = ref(0);
const ITEM_HEIGHT = 48;
const VIEWPORT_HEIGHT = 520;
const BUFFER_SIZE = 5;

let pollTimer: ReturnType<typeof setInterval> | null = null;
let unregisterStop: (() => void) | null = null;

function getAuthHeaders(): HeadersInit {
  const session = useSessionStore();
  const headers: Record<string, string> = {};
  if (session.token) {
    headers.Authorization = `Bearer ${session.token.trim()}`;
  }
  return headers;
}

async function fetchFrames() {
  loading.value = true;
  try {
    const res = await fetch('/v1/telemetry/recorder', {
      headers: getAuthHeaders(),
    });
    if (res.ok) {
      const data = (await res.json()) as RecordedFrame[];
      // Keep most recent first
      frames.value = data.slice().reverse();
    }
  } catch {
    // network or backend failure
  } finally {
    loading.value = false;
  }
}

const filteredFrames = computed(() => {
  return frames.value.filter((f) => {
    if (filterEndpoint.value && !f.endpoint.toLowerCase().includes(filterEndpoint.value.toLowerCase())) {
      return false;
    }
    if (filterProvider.value !== 'all' && (f.provider || 'unknown') !== filterProvider.value) {
      return false;
    }
    if (filterStatus.value !== 'all') {
      const code = f.status_code ?? 0;
      if (filterStatus.value === '2xx' && (code < 200 || code >= 300)) return false;
      if (filterStatus.value === '4xx' && (code < 400 || code >= 500)) return false;
      if (filterStatus.value === '5xx' && (code < 500 || code >= 600)) return false;
    }
    return true;
  });
});

const totalHeight = computed(() => filteredFrames.value.length * ITEM_HEIGHT);

const visibleIndices = computed(() => {
  const start = Math.max(0, Math.floor(scrollTop.value / ITEM_HEIGHT) - BUFFER_SIZE);
  const visibleCount = Math.ceil(VIEWPORT_HEIGHT / ITEM_HEIGHT) + 2 * BUFFER_SIZE;
  const end = Math.min(filteredFrames.value.length, start + visibleCount);
  return { start, end };
});

const visibleItems = computed(() => {
  const { start, end } = visibleIndices.value;
  return filteredFrames.value.slice(start, end).map((frame, i) => ({
    frame,
    index: start + i,
    top: (start + i) * ITEM_HEIGHT,
  }));
});

function handleScroll(e: Event) {
  const target = e.target as HTMLDivElement;
  scrollTop.value = target.scrollTop;
}

function openFrame(frame: RecordedFrame, idx: number) {
  selectedIndex.value = idx;
  activeFrame.value = frame;
  isDrawerOpen.value = true;
}

function closeDrawer() {
  isDrawerOpen.value = false;
}

function handleKeyDown(e: KeyboardEvent) {
  // If user is focusing an input, don't hijack j/k
  if (['INPUT', 'SELECT', 'TEXTAREA'].includes((e.target as HTMLElement)?.tagName)) {
    return;
  }

  const list = filteredFrames.value;
  if (list.length === 0) return;

  if (e.key === 'j') {
    e.preventDefault();
    selectedIndex.value = Math.min(list.length - 1, selectedIndex.value + 1);
    ensureVisible(selectedIndex.value);
  } else if (e.key === 'k') {
    e.preventDefault();
    selectedIndex.value = Math.max(0, selectedIndex.value - 1);
    ensureVisible(selectedIndex.value);
  } else if (e.key === 'Enter') {
    if (selectedIndex.value >= 0 && selectedIndex.value < list.length) {
      e.preventDefault();
      openFrame(list[selectedIndex.value], selectedIndex.value);
    }
  }
}

function ensureVisible(index: number) {
  if (!viewportRef.value) return;
  const itemTop = index * ITEM_HEIGHT;
  const itemBottom = itemTop + ITEM_HEIGHT;
  const viewTop = viewportRef.value.scrollTop;
  const viewBottom = viewTop + VIEWPORT_HEIGHT;

  if (itemTop < viewTop) {
    viewportRef.value.scrollTop = itemTop;
  } else if (itemBottom > viewBottom) {
    viewportRef.value.scrollTop = itemBottom - VIEWPORT_HEIGHT;
  }
}

onMounted(() => {
  void fetchFrames();
  pollTimer = setInterval(() => {
    if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return;
    void fetchFrames();
  }, 2000);

  unregisterStop = onStopPolling(() => {
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
  });

  window.addEventListener('keydown', handleKeyDown);
});

onUnmounted(() => {
  if (pollTimer) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
  unregisterStop?.();
  window.removeEventListener('keydown', handleKeyDown);
});
</script>

<template>
  <div class="recorder-page">
    <NavBar />

    <main class="page-content">
      <div class="header-bar">
        <div>
          <h1 class="page-title">黑匣子录波 (Flight Recorder)</h1>
          <p class="page-desc">最近 200 帧端到端请求详情（支持 j/k 键盘切帧，Enter 展开详情）</p>
        </div>
        <button class="refresh-btn" :disabled="loading" @click="fetchFrames">
          {{ loading ? '拉取中...' : '刷新录波' }}
        </button>
      </div>

      <!-- Filter Bar -->
      <div class="filter-bar">
        <div class="filter-item">
          <label>端点筛选:</label>
          <input
            v-model="filterEndpoint"
            type="text"
            placeholder="搜索 /v1/..."
            class="input-search"
          >
        </div>

        <div class="filter-item">
          <label>状态筛选:</label>
          <select v-model="filterStatus" class="select-box">
            <option value="all">全部状态</option>
            <option value="2xx">2xx 成功</option>
            <option value="4xx">4xx 客户端异常</option>
            <option value="5xx">5xx 服务端错误</option>
          </select>
        </div>

        <div class="filter-item">
          <label>Provider:</label>
          <select v-model="filterProvider" class="select-box">
            <option value="all">全部 Provider</option>
            <option value="deepseek">deepseek</option>
            <option value="openai">openai</option>
            <option value="claude">claude</option>
          </select>
        </div>

        <div class="filter-summary">
          共 {{ filteredFrames.length }} / {{ frames.length }} 帧
        </div>
      </div>

      <!-- Table Header -->
      <div class="table-header">
        <div class="col-status">状态</div>
        <div class="col-latency">耗时</div>
        <div class="col-endpoint">端点</div>
        <div class="col-provider">Provider</div>
        <div class="col-key">脱敏 Key</div>
        <div class="col-time">时间</div>
      </div>

      <!-- Virtual Scroll Viewport -->
      <div
        ref="viewportRef"
        class="virtual-viewport"
        :style="{ height: `${VIEWPORT_HEIGHT}px` }"
        @scroll="handleScroll"
      >
        <div class="scroll-phantom" :style="{ height: `${totalHeight}px` }" />

        <div class="items-layer">
          <div
            v-for="item in visibleItems"
            :key="item.frame.request_id"
            class="table-row"
            :class="{
              selected: selectedIndex === item.index,
              'has-error': (item.frame.status_code ?? 0) >= 400,
            }"
            :style="{
              height: `${ITEM_HEIGHT}px`,
              transform: `translateY(${item.top}px)`,
            }"
            @click="openFrame(item.frame, item.index)"
          >
            <div class="col-status">
              <span class="badge" :class="(item.frame.status_code ?? 0) < 400 ? 'status-ok' : 'status-err'">
                {{ item.frame.status_code ?? '--' }}
              </span>
            </div>
            <div class="col-latency">{{ item.frame.latency_ms }} ms</div>
            <div class="col-endpoint font-mono">{{ item.frame.endpoint }}</div>
            <div class="col-provider">{{ item.frame.provider || '--' }}</div>
            <div class="col-key font-mono text-masked">
              {{ maskKey(item.frame.sanitized_key) }}
            </div>
            <div class="col-time">
              {{ new Date(item.frame.timestamp).toLocaleTimeString() }}
            </div>
          </div>
        </div>

        <div v-if="filteredFrames.length === 0" class="empty-state">
          暂无匹配的录波帧
        </div>
      </div>
    </main>

    <!-- Frame Details Drawer -->
    <FrameDrawer
      :frame="activeFrame"
      :is-open="isDrawerOpen"
      @close="closeDrawer"
    />
  </div>
</template>

<style scoped>
.recorder-page {
  min-height: 100vh;
  background: #f8fafc;
  font-family: system-ui, -apple-system, sans-serif;
}

.page-content {
  max-width: 1280px;
  margin: 0 auto;
  padding: 24px;
}

.header-bar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 20px;
}

.page-title {
  font-size: 20px;
  font-weight: 700;
  color: #0f172a;
  margin: 0 0 4px 0;
}

.page-desc {
  font-size: 13px;
  color: #64748b;
  margin: 0;
}

.refresh-btn {
  padding: 8px 16px;
  background: #2563eb;
  color: #ffffff;
  border: none;
  border-radius: 6px;
  font-size: 13px;
  font-weight: 500;
  cursor: pointer;
}

.refresh-btn:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

.filter-bar {
  display: flex;
  align-items: center;
  gap: 20px;
  background: #ffffff;
  padding: 12px 16px;
  border-radius: 8px;
  border: 1px solid #e2e8f0;
  margin-bottom: 16px;
  font-size: 13px;
}

.filter-item {
  display: flex;
  align-items: center;
  gap: 8px;
  color: #475569;
}

.input-search, .select-box {
  padding: 6px 10px;
  border: 1px solid #cbd5e1;
  border-radius: 6px;
  font-size: 13px;
  color: #1e293b;
  outline: none;
}

.filter-summary {
  margin-left: auto;
  color: #94a3b8;
  font-size: 12px;
}

.table-header {
  display: flex;
  align-items: center;
  padding: 10px 16px;
  background: #f1f5f9;
  border: 1px solid #e2e8f0;
  border-bottom: none;
  border-radius: 8px 8px 0 0;
  font-size: 12px;
  font-weight: 600;
  color: #64748b;
}

.virtual-viewport {
  position: relative;
  overflow-y: auto;
  background: #ffffff;
  border: 1px solid #e2e8f0;
  border-radius: 0 0 8px 8px;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.04);
}

.scroll-phantom {
  position: absolute;
  left: 0;
  top: 0;
  right: 0;
  pointer-events: none;
}

.items-layer {
  position: absolute;
  left: 0;
  top: 0;
  right: 0;
}

.table-row {
  position: absolute;
  left: 0;
  right: 0;
  display: flex;
  align-items: center;
  padding: 0 16px;
  border-bottom: 1px solid #f1f5f9;
  font-size: 13px;
  color: #334155;
  cursor: pointer;
  transition: background 0.1s ease;
  user-select: none;
}

.table-row:hover {
  background: #f8fafc;
}

.table-row.selected {
  background: #eff6ff;
  border-left: 3px solid #2563eb;
}

.col-status { width: 80px; flex-shrink: 0; }
.col-latency { width: 90px; flex-shrink: 0; }
.col-endpoint { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.col-provider { width: 110px; flex-shrink: 0; }
.col-key { width: 110px; flex-shrink: 0; }
.col-time { width: 100px; flex-shrink: 0; text-align: right; color: #94a3b8; }

.font-mono { font-family: monospace; }
.text-masked { color: #0284c7; font-weight: 500; }

.badge {
  font-size: 11px;
  padding: 2px 6px;
  border-radius: 4px;
  font-weight: 600;
}

.status-ok { background: #dcfce7; color: #166534; }
.status-err { background: #fee2e2; color: #991b1b; }

.empty-state {
  display: flex;
  justify-content: center;
  align-items: center;
  height: 200px;
  color: #94a3b8;
  font-size: 14px;
}
</style>
