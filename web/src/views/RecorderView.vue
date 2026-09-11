<script setup lang="ts">
import { ref, computed, watch, onMounted, onUnmounted, nextTick } from 'vue';
import NavBar from '../components/NavBar.vue';
import FrameDrawer from '../components/FrameDrawer.vue';
import Icons from '../components/ui/Icons.vue';
import UiButton from '../components/ui/UiButton.vue';
import UiTooltip from '../components/ui/UiTooltip.vue';
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

// 分页状态
const currentPage = ref(1);
const pageSize = ref(20);
const pageSizeOptions = [20, 50, 100];

const tableContainerRef = ref<HTMLDivElement | null>(null);
const listRowsRef = ref<HTMLTableRowElement[]>([]);

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
    // 列表模式默认拉取极轻量摘要（不带超大 request/response snippet），瞬时秒开
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

// 动态提取已知 provider 列表
const availableProviders = computed(() => {
  const set = new Set<string>();
  for (const f of frames.value) {
    if (f.provider) {
      set.add(f.provider);
    }
  }
  return Array.from(set);
});

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

const totalPages = computed(() => Math.max(1, Math.ceil(filteredFrames.value.length / pageSize.value)));

const paginatedFrames = computed(() => {
  const start = (currentPage.value - 1) * pageSize.value;
  return filteredFrames.value.slice(start, start + pageSize.value);
});

// 当筛选条件变化或每页条数变化时重置页码
watch([filteredFrames, pageSize], () => {
  if (currentPage.value > totalPages.value) {
    currentPage.value = totalPages.value;
  }
  if (currentPage.value < 1) {
    currentPage.value = 1;
  }
  selectedIndex.value = -1;
});

async function openFrame(frame: RecordedFrame, idx: number) {
  selectedIndex.value = idx;
  activeFrame.value = frame;
  isDrawerOpen.value = true;

  // 懒加载模式：如果当前只有摘要帧，异步按需拉取对应 request_id 的完整全量帧（含超大 payload 与全量 response）
  if (!frame.request_snippet && !frame.response_snippet) {
    try {
      const res = await fetch(`/v1/telemetry/recorder/${encodeURIComponent(frame.request_id)}`, {
        headers: getAuthHeaders(),
      });
      if (res.ok) {
        const full = (await res.json()) as RecordedFrame;
        if (activeFrame.value?.request_id === frame.request_id) {
          activeFrame.value = full;
        }
        // 回写缓存
        const listIdx = frames.value.findIndex((f) => f.request_id === frame.request_id);
        if (listIdx !== -1) {
          frames.value[listIdx] = full;
        }
      }
    } catch {
      // 容错保留原有摘要
    }
  }
}

function closeDrawer() {
  isDrawerOpen.value = false;
}

function changePage(page: number) {
  if (page < 1 || page > totalPages.value || page === currentPage.value) return;
  currentPage.value = page;
  selectedIndex.value = -1;
  if (tableContainerRef.value) {
    tableContainerRef.value.scrollTop = 0;
  }
}

function scrollToSelectedRow(index: number) {
  nextTick(() => {
    const rowEl = listRowsRef.value[index];
    if (rowEl && tableContainerRef.value) {
      const container = tableContainerRef.value;
      const rowTop = rowEl.offsetTop;
      const rowBottom = rowTop + rowEl.offsetHeight;
      const viewTop = container.scrollTop;
      const viewBottom = viewTop + container.clientHeight;

      if (rowTop < viewTop) {
        container.scrollTop = rowTop;
      } else if (rowBottom > viewBottom) {
        container.scrollTop = rowBottom - container.clientHeight;
      }
    }
  });
}

function handleKeyDown(e: KeyboardEvent) {
  // 忽略系统修饰组合键 (如 Ctrl+Up, Cmd+Down)
  if (e.ctrlKey || e.metaKey || e.altKey) {
    return;
  }

  // 抽屉展开时挂起快捷键，防止穿透
  if (isDrawerOpen.value) {
    return;
  }

  // 输入框或选择器聚焦时，不劫持上下键
  if (['INPUT', 'SELECT', 'TEXTAREA'].includes((e.target as HTMLElement)?.tagName)) {
    return;
  }

  const list = paginatedFrames.value;
  if (list.length === 0) return;

  if (e.key === 'ArrowDown') {
    e.preventDefault();
    if (selectedIndex.value < list.length - 1) {
      selectedIndex.value += 1;
    } else if (currentPage.value < totalPages.value) {
      // 跨页向下
      currentPage.value += 1;
      selectedIndex.value = 0;
    }
    scrollToSelectedRow(selectedIndex.value);
  } else if (e.key === 'ArrowUp') {
    e.preventDefault();
    if (selectedIndex.value > 0) {
      selectedIndex.value -= 1;
    } else if (selectedIndex.value === 0 && currentPage.value > 1) {
      // 跨页向上
      currentPage.value -= 1;
      selectedIndex.value = pageSize.value - 1;
    } else if (selectedIndex.value === -1) {
      selectedIndex.value = 0;
    }
    scrollToSelectedRow(selectedIndex.value);
  } else if (e.key === 'Enter') {
    if (selectedIndex.value >= 0 && selectedIndex.value < list.length) {
      e.preventDefault();
      openFrame(list[selectedIndex.value], selectedIndex.value);
    }
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
  <div class="min-h-screen flex flex-col bg-transparent text-slate-900">
    <NavBar />

    <main class="flex-1 min-h-0 w-full max-w-[1600px] mx-auto px-4 sm:px-6 py-6 flex flex-col">
      <!-- 页面头部：毛玻璃标题与操作栏 -->
      <div class="flex flex-col sm:flex-row sm:items-center justify-between gap-4 mb-5 shrink-0">
        <div>
          <h1 class="text-2xl font-bold tracking-tight text-slate-900 flex items-center gap-2.5">
            轨迹
          </h1>
          <p class="text-sm text-slate-500 mt-1">
            保留 7 天端到端请求详情（支持上下键切帧，Enter 展开详情）
          </p>
        </div>

        <div class="flex items-center gap-2 self-start sm:self-auto">
          <UiTooltip content="刷新远端轨迹">
            <UiButton
              variant="outline"
              size="sm"
              :disabled="loading"
              class="refresh-btn"
              @click="fetchFrames"
            >
              <Icons name="refresh" size="14" :class="loading ? 'animate-spin' : ''" />
              {{ loading ? '拉取中...' : '刷新轨迹' }}
            </UiButton>
          </UiTooltip>
        </div>
      </div>

      <!-- Filter Bar: 毛玻璃过滤条 -->
      <div class="swiss-card p-3.5 sm:p-4 mb-4 flex flex-wrap items-center gap-3 sm:gap-4 text-sm shrink-0">
        <div class="flex items-center gap-2">
          <span class="text-slate-600 font-medium text-xs sm:text-sm whitespace-nowrap">端点:</span>
          <input
            v-model="filterEndpoint"
            type="text"
            placeholder="搜索 /v1/..."
            class="input-search text-xs sm:text-sm bg-white/70 hover:bg-white focus:bg-white border border-slate-200/80 rounded-lg px-3 py-1.5 text-slate-900 placeholder:text-slate-400 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-400 transition-all duration-150 w-36 sm:w-48"
          />
        </div>

        <div class="flex items-center gap-2">
          <span class="text-slate-600 font-medium text-xs sm:text-sm whitespace-nowrap">状态:</span>
          <select
            v-model="filterStatus"
            class="select-box text-xs sm:text-sm bg-white/70 hover:bg-white focus:bg-white border border-slate-200/80 rounded-lg px-2.5 py-1.5 text-slate-900 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-400 transition-all duration-150 cursor-pointer"
          >
            <option value="all">全部状态</option>
            <option value="2xx">2xx 成功</option>
            <option value="4xx">4xx 客户端异常</option>
            <option value="5xx">5xx 服务端错误</option>
          </select>
        </div>

        <div class="flex items-center gap-2">
          <span class="text-slate-600 font-medium text-xs sm:text-sm whitespace-nowrap">Provider:</span>
          <select
            v-model="filterProvider"
            class="select-box text-xs sm:text-sm bg-white/70 hover:bg-white focus:bg-white border border-slate-200/80 rounded-lg px-2.5 py-1.5 text-slate-900 focus:outline-none focus:ring-2 focus:ring-slate-400/20 focus:border-slate-400 transition-all duration-150 cursor-pointer"
          >
            <option value="all">全部 Provider</option>
            <option v-for="prov in availableProviders" :key="prov" :value="prov">
              {{ prov }}
            </option>
          </select>
        </div>

        <div class="ml-auto text-slate-500 font-mono text-xs sm:text-sm font-medium">
          共 {{ filteredFrames.length }} / {{ frames.length }} 帧
        </div>
      </div>

      <!-- 主列表容器：毛玻璃卡片包裹，撑满视口剩余高度，可纵横滚动无右侧滚动条 -->
      <div class="swiss-card flex-1 min-h-0 flex flex-col overflow-hidden border border-white/40 shadow-xs">
        <!-- 宽表格容器：支持横向宽展铺开与无滚动条纵向滚动 -->
        <div
          ref="tableContainerRef"
          class="flex-1 min-h-0 overflow-auto no-scrollbar scrollbar-none"
        >
          <table class="w-full min-w-[1000px] border-collapse text-left text-sm">
            <!-- 固顶表头 -->
            <thead class="sticky top-0 z-10 bg-slate-100/90 backdrop-blur-md border-b border-slate-200/80 text-xs font-semibold text-slate-600 select-none shadow-2xs">
              <tr>
                <th class="py-3.5 pl-8 pr-4 w-28">状态</th>
                <th class="py-3.5 px-4 w-28">耗时</th>
                <th class="py-3.5 px-4 min-w-[220px]">端点</th>
                <th class="py-3.5 px-4 w-32">Provider</th>
                <th class="py-3.5 px-4 w-36">Key 标识</th>
                <th class="py-3.5 px-4 min-w-[200px]">摘要 / Payload</th>
                <th class="py-3.5 pl-4 pr-8 w-40 text-right">时间</th>
              </tr>
            </thead>

            <tbody class="divide-y divide-slate-100/80">
              <tr
                v-for="(frame, idx) in paginatedFrames"
                :key="frame.request_id"
                :ref="(el) => { if (el) listRowsRef[idx] = el as HTMLTableRowElement; }"
                class="table-row cursor-pointer transition-colors duration-100 select-none group"
                :class="{
                  '!bg-slate-900/10 font-medium border-l-4 border-l-slate-900': selectedIndex === idx,
                  'hover:bg-slate-900/5': selectedIndex !== idx,
                  'bg-rose-50/40': (frame.status_code ?? 0) >= 400 && selectedIndex !== idx,
                }"
                @click="openFrame(frame, idx)"
              >
                <!-- 状态 -->
                <td class="py-3.5 pl-8 pr-4 whitespace-nowrap">
                  <span
                    class="badge inline-flex items-center px-2 py-0.5 rounded font-mono text-xs font-semibold"
                    :class="(frame.status_code ?? 0) < 400 ? 'status-ok' : 'status-err'"
                  >
                    {{ frame.status_code ?? '--' }}
                  </span>
                </td>

                <!-- 耗时 -->
                <td class="py-3.5 px-4 whitespace-nowrap font-mono text-xs text-slate-700">
                  {{ frame.latency_ms }} ms
                </td>

                <!-- 端点 -->
                <td class="py-3.5 px-4 font-mono text-xs text-slate-900">
                  <div class="truncate max-w-[320px]" :title="frame.endpoint">
                    {{ frame.endpoint }}
                  </div>
                </td>

                <!-- Provider -->
                <td class="py-3.5 px-4 whitespace-nowrap text-xs text-slate-600">
                  <span class="inline-flex items-center px-2 py-0.5 rounded bg-slate-100/80 text-slate-700 font-medium">
                    {{ frame.provider || '--' }}
                  </span>
                </td>

                <!-- Key 标识 (无需脱敏) -->
                <td class="py-3.5 px-4 whitespace-nowrap font-mono text-xs text-slate-800 font-medium">
                  <span :title="frame.key_id">
                    {{ frame.key_id || '--' }}
                  </span>
                </td>

                <!-- 摘要 / 响应概览 -->
                <td class="py-3.5 px-4 text-xs font-mono text-slate-500">
                  <div class="truncate max-w-[280px]" :title="frame.error || frame.request_snippet || frame.response_snippet || '--'">
                    <span v-if="frame.error" class="text-rose-600 font-semibold">
                      [Err] {{ frame.error }}
                    </span>
                    <span v-else-if="frame.stream_flow?.chunks" class="text-emerald-700">
                      [Stream] {{ frame.stream_flow.chunks }} chunks / {{ frame.stream_flow.ttft_ms ?? '--' }}ms ttft
                    </span>
                    <span v-else>
                      {{ frame.request_snippet || frame.response_snippet || '--' }}
                    </span>
                  </div>
                </td>

                <!-- 时间 -->
                <td class="py-3.5 pl-4 pr-8 whitespace-nowrap text-right font-mono text-xs text-slate-500">
                  {{ new Date(frame.timestamp).toLocaleTimeString() }}
                </td>
              </tr>
            </tbody>
          </table>

          <!-- 空态 -->
          <div v-if="filteredFrames.length === 0" class="flex flex-col items-center justify-center h-48 text-slate-400 text-sm">
            <Icons name="info" size="24" class="mb-2 opacity-50" />
            <span>暂无匹配的轨迹帧</span>
          </div>
        </div>

        <!-- 底部毛玻璃分页组件 -->
        <div class="shrink-0 flex flex-wrap items-center justify-between gap-3 px-8 py-3 bg-white/50 backdrop-blur-xs border-t border-slate-200/60 text-xs text-slate-600 select-none">
          <div class="flex items-center gap-2">
            <span>每页显示:</span>
            <select
              v-model="pageSize"
              class="bg-white/80 border border-slate-200 rounded px-2 py-1 text-xs text-slate-800 focus:outline-none focus:ring-1 focus:ring-slate-400 cursor-pointer"
            >
              <option v-for="opt in pageSizeOptions" :key="opt" :value="opt">
                {{ opt }} 条
              </option>
            </select>
            <span class="text-slate-400 ml-2">
              显示第 {{ filteredFrames.length > 0 ? (currentPage - 1) * pageSize + 1 : 0 }} - {{ Math.min(currentPage * pageSize, filteredFrames.length) }} 条
            </span>
          </div>

          <div class="flex items-center gap-1.5 ml-auto">
            <UiButton
              variant="outline"
              size="sm"
              :disabled="currentPage <= 1"
              class="h-7 px-2.5 text-xs text-slate-700 bg-white/70"
              @click="changePage(currentPage - 1)"
            >
              <Icons name="chevron-left" size="14" />
              上一页
            </UiButton>

            <span class="px-2 font-mono text-xs text-slate-700">
              {{ currentPage }} / {{ totalPages }}
            </span>

            <UiButton
              variant="outline"
              size="sm"
              :disabled="currentPage >= totalPages"
              class="h-7 px-2.5 text-xs text-slate-700 bg-white/70"
              @click="changePage(currentPage + 1)"
            >
              下一页
              <Icons name="chevron-right" size="14" />
            </UiButton>
          </div>
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
.badge {
  letter-spacing: 0.02em;
}

.status-ok {
  background: rgba(220, 252, 231, 0.9);
  color: #166534;
  border: 1px solid #bbf7d0;
}

.status-err {
  background: rgba(254, 226, 226, 0.9);
  color: #991b1b;
  border: 1px solid #fecaca;
}
</style>
