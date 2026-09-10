<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from 'vue';
import * as echarts from 'echarts/core';
import { LineChart, BarChart } from 'echarts/charts';
import { GridComponent, TooltipComponent, LegendComponent } from 'echarts/components';
import { CanvasRenderer } from 'echarts/renderers';
import Icons from './ui/Icons.vue';
import UiTooltip from './ui/UiTooltip.vue';
import type { TelemetryPoint } from '../composables/useTelemetry';
import type { TimeseriesHistoryResponse } from '../types/telemetry';

echarts.use([LineChart, BarChart, GridComponent, TooltipComponent, LegendComponent, CanvasRenderer]);

const props = withDefaults(
  defineProps<{
    history?: TelemetryPoint[];
    historyData?: TimeseriesHistoryResponse | null;
    range?: '24h' | '7d' | '30d';
  }>(),
  {
    history: () => [],
    historyData: null,
    range: '24h',
  }
);

const emit = defineEmits<{
  (e: 'update:range', range: '24h' | '7d' | '30d'): void;
}>();

const rangeOptions: Array<{ key: '24h' | '7d' | '30d'; label: string }> = [
  { key: '24h', label: '24小时' },
  { key: '7d', label: '7天' },
  { key: '30d', label: '30天' },
];

const qpsChartRef = ref<HTMLDivElement | null>(null);
const tokenChartRef = ref<HTMLDivElement | null>(null);
const latencyChartRef = ref<HTMLDivElement | null>(null);
const errorChartRef = ref<HTMLDivElement | null>(null);

let qpsChart: echarts.ECharts | null = null;
let tokenChart: echarts.ECharts | null = null;
let latencyChart: echarts.ECharts | null = null;
let errorChart: echarts.ECharts | null = null;

function formatTimestamp(ts: number, range: string): string {
  const d = new Date(ts);
  if (range === '30d' || range === '7d') {
    return `${(d.getMonth() + 1).toString().padStart(2, '0')}/${d.getDate().toString().padStart(2, '0')}`;
  }
  return `${d.getHours().toString().padStart(2, '0')}:${d.getMinutes().toString().padStart(2, '0')}`;
}

function getXAxisLabelConfig() {
  const r = props.range;
  return {
    color: '#64748b',
    fontSize: 12,
    hideOverlap: true,
    interval: r === '30d' ? 4 : r === '7d' ? 3 : (index: number) => index % 2 === 0,
  };
}

const commonTooltip = {
  trigger: 'axis' as const,
  backgroundColor: 'rgba(15, 23, 42, 0.92)',
  borderColor: '#334155',
  borderWidth: 1,
  padding: [8, 12],
  textStyle: {
    color: '#ffffff',
    fontSize: 12,
  },
};

const commonGrid = { top: 24, right: 16, bottom: 24, left: 44 };

// Colors
const COLOR_WARM_ORANGE = '#f97316';
const COLOR_SLATE_CYAN = '#0284c7';
const COLOR_TEAL = '#0d9488';
const COLOR_ROSE = '#e11d48';

function formatAxisNumber(val: number): string {
  if (val >= 1_000_000) return `${Math.round(val / 1_000_000)}M`;
  if (val >= 1_000) return `${Math.round(val / 1_000)}K`;
  return Math.round(val).toString();
}

function updateCharts() {
  const points = props.historyData?.points || [];
  const useHistorical = points.length > 0;

  // Timestamps
  const timestamps = useHistorical
    ? points.map((p) => formatTimestamp(p.timestamp_ms, props.range))
    : props.history.map((h) => formatTimestamp(h.timestamp, '24h'));

  // 1. QPS 折线面积图 (Smooth Line + Area)
  const qpsData = useHistorical ? points.map((p) => p.qps) : props.history.map((h) => h.qps);
  qpsChart?.setOption({
    tooltip: commonTooltip,
    grid: commonGrid,
    xAxis: {
      type: 'category',
      data: timestamps,
      boundaryGap: false,
      axisLine: { lineStyle: { color: '#e2e8f0' } },
      axisTick: { show: false },
      axisLabel: getXAxisLabelConfig(),
    },
    yAxis: {
      type: 'value',
      min: 0,
      splitNumber: 4,
      splitLine: { lineStyle: { color: '#f1f5f9' } },
      axisLabel: {
        color: '#64748b',
        fontSize: 12,
        formatter: (val: number, idx: number) => (idx % 2 === 0 ? formatAxisNumber(val) : ''),
      },
    },
    series: [
      {
        name: 'QPS',
        type: 'line',
        smooth: 0.35,
        showSymbol: false,
        data: qpsData,
        areaStyle: {
          color: new echarts.graphic.LinearGradient(0, 0, 0, 1, [
            { offset: 0, color: 'rgba(249, 115, 22, 0.28)' },
            { offset: 1, color: 'rgba(249, 115, 22, 0.01)' },
          ]),
        },
        itemStyle: { color: COLOR_WARM_ORANGE },
        lineStyle: { width: 2.5 },
      },
    ],
  });

  // 2. Token 吞吐量 柱状分布图 (全量类型堆叠图: 输入 / 输出 / 缓存命中)
  if (useHistorical) {
    const barSeries = [
      {
        name: '输入 Token',
        type: 'bar' as const,
        stack: 'total',
        barMaxWidth: 18,
        itemStyle: { color: '#64748b', borderRadius: [0, 0, 0, 0] },
        data: points.map((p) => p.prompt_tokens ?? 0),
      },
      {
        name: '输出 Token',
        type: 'bar' as const,
        stack: 'total',
        barMaxWidth: 18,
        itemStyle: { color: '#0284c7', borderRadius: [0, 0, 0, 0] },
        data: points.map((p) => p.completion_tokens ?? 0),
      },
      {
        name: '缓存命中',
        type: 'bar' as const,
        stack: 'total',
        barMaxWidth: 18,
        itemStyle: { color: '#10b981', borderRadius: [2, 2, 0, 0] },
        data: points.map((p) => p.cached_tokens ?? 0),
      },
    ];

    tokenChart?.setOption({
      tooltip: {
        ...commonTooltip,
        formatter: (params: any) => {
          if (!Array.isArray(params) || params.length === 0) return '';
          const idx = params[0].dataIndex;
          const pt = points[idx];
          const time = params[0].axisValueLabel || '';
          let html = `<div style="font-weight:600;margin-bottom:4px">${time}</div>`;
          if (pt) {
            html += `<div style="font-size:11px;color:#94a3b8;margin-bottom:6px">切片请求: ${pt.total_requests.toLocaleString()} 次</div>`;
          }
          let totalTokens = 0;
          for (const item of params) {
            const val = Number(item.value) || 0;
            totalTokens += val;
            const formattedInt = formatAxisNumber(val);
            let pctSuffix = '';
            if (item.seriesName === '缓存命中' && pt) {
              const promptVal = pt.prompt_tokens ?? 0;
              const cachedVal = pt.cached_tokens ?? val;
              if (promptVal + cachedVal > 0) {
                const pct = Math.round((cachedVal / (promptVal + cachedVal)) * 100);
                pctSuffix = ` [${pct}%]`;
              }
            }
            html += `<div style="display:flex;align-items:center;justify-content:space-between;gap:12px;font-size:12px">
              <span>${item.marker} ${item.seriesName}</span>
              <span style="font-weight:600;font-family:var(--font-mono-family, monospace);font-variant-numeric:tabular-nums">${formattedInt}${pctSuffix} (${val.toLocaleString()} tok)</span>
            </div>`;
          }
          if (params.length > 1) {
            html += `<div style="border-top:1px solid #334155;margin-top:4px;padding-top:4px;display:flex;justify-content:space-between;font-size:12px;font-weight:600">
              <span>合计</span>
              <span style="font-family:var(--font-mono-family, monospace);font-variant-numeric:tabular-nums">${formatAxisNumber(totalTokens)} (${totalTokens.toLocaleString()} tok)</span>
            </div>`;
          }
          return html;
        },
      },
      legend: {
        show: true,
        top: 0,
        right: 0,
        textStyle: { fontSize: 12, color: '#64748b' },
        itemWidth: 10,
        itemHeight: 10,
      },
      grid: { ...commonGrid, top: 32 },
      xAxis: {
        type: 'category',
        data: timestamps,
        axisLine: { lineStyle: { color: '#e2e8f0' } },
        axisTick: { show: false },
        axisLabel: getXAxisLabelConfig(),
      },
      yAxis: {
        type: 'value',
        min: 0,
        splitNumber: 4,
        splitLine: { lineStyle: { color: '#f1f5f9' } },
        axisLabel: {
          color: '#64748b',
          fontSize: 12,
          formatter: (val: number, idx: number) => (idx % 2 === 0 ? formatAxisNumber(val) : ''),
        },
      },
      series: barSeries,
    }, { notMerge: true });
  } else {
    // Realtime fallback multi-series (Prompt / Completion / Cached)
    const promptData = props.history.map((h) => h.promptThroughput ?? 0);
    const compData = props.history.map((h) => h.completionThroughput ?? (h.tokenThroughput ?? 0));
    const cachedData = props.history.map((h) => h.cachedThroughput ?? 0);
    tokenChart?.setOption({
      tooltip: commonTooltip,
      legend: {
        show: true,
        top: 0,
        right: 0,
        textStyle: { fontSize: 12, color: '#64748b' },
        itemWidth: 10,
        itemHeight: 10,
      },
      grid: { ...commonGrid, top: 32 },
      xAxis: {
        type: 'category',
        data: timestamps,
        axisLine: { lineStyle: { color: '#e2e8f0' } },
        axisTick: { show: false },
        axisLabel: getXAxisLabelConfig(),
      },
      yAxis: {
        type: 'value',
        min: 0,
        splitNumber: 4,
        splitLine: { lineStyle: { color: '#f1f5f9' } },
        axisLabel: {
          color: '#64748b',
          fontSize: 12,
          formatter: (val: number, idx: number) => (idx % 2 === 0 ? formatAxisNumber(val) : ''),
        },
      },
      series: [
        {
          name: '输入 (tok/s)',
          type: 'bar',
          stack: 'realtime',
          barMaxWidth: 16,
          itemStyle: { color: '#64748b' },
          data: promptData,
        },
        {
          name: '输出 (tok/s)',
          type: 'bar',
          stack: 'realtime',
          barMaxWidth: 16,
          itemStyle: { color: '#0284c7' },
          data: compData,
        },
        {
          name: '缓存 (tok/s)',
          type: 'bar',
          stack: 'realtime',
          barMaxWidth: 16,
          itemStyle: { color: '#10b981', borderRadius: [2, 2, 0, 0] },
          data: cachedData,
        },
      ],
    }, { notMerge: true });
  }

  // 3. 延迟与速率 波形图 (Waveform / Spline Area)
  const latencyData = useHistorical ? points.map((p) => p.avg_latency_ms) : props.history.map((h) => h.latencyMs);
  latencyChart?.setOption({
    tooltip: commonTooltip,
    grid: commonGrid,
    xAxis: {
      type: 'category',
      data: timestamps,
      boundaryGap: false,
      axisLine: { lineStyle: { color: '#e2e8f0' } },
      axisTick: { show: false },
      axisLabel: getXAxisLabelConfig(),
    },
    yAxis: {
      type: 'value',
      min: 0,
      splitNumber: 4,
      splitLine: { lineStyle: { color: '#f1f5f9' } },
      axisLabel: {
        color: '#64748b',
        fontSize: 12,
        formatter: (val: number, idx: number) => (idx % 2 === 0 ? formatAxisNumber(val) : ''),
      },
    },
    series: [
      {
        name: '平均延迟 (ms)',
        type: 'line',
        smooth: 0.5,
        showSymbol: false,
        data: latencyData,
        areaStyle: {
          color: new echarts.graphic.LinearGradient(0, 0, 0, 1, [
            { offset: 0, color: 'rgba(13, 148, 136, 0.28)' },
            { offset: 1, color: 'rgba(13, 148, 136, 0.01)' },
          ]),
        },
        itemStyle: { color: COLOR_TEAL },
        lineStyle: { width: 2.2 },
      },
    ],
  });

  // 4. 故障率 微波阶梯图 (Step Line)
  const errorData = useHistorical ? points.map((p) => p.error_rate) : props.history.map((h) => h.errorRate);
  errorChart?.setOption({
    tooltip: commonTooltip,
    grid: commonGrid,
    xAxis: {
      type: 'category',
      data: timestamps,
      boundaryGap: false,
      axisLine: { lineStyle: { color: '#e2e8f0' } },
      axisTick: { show: false },
      axisLabel: getXAxisLabelConfig(),
    },
    yAxis: {
      type: 'value',
      min: 0,
      max: 100,
      splitNumber: 4,
      splitLine: { lineStyle: { color: '#f1f5f9' } },
      axisLabel: {
        color: '#64748b',
        fontSize: 12,
        formatter: (val: number, idx: number) => (idx % 2 === 0 ? `${Math.round(val)}%` : ''),
      },
    },
    series: [
      {
        name: '故障率 (%)',
        type: 'line',
        step: 'middle',
        showSymbol: false,
        data: errorData,
        areaStyle: {
          color: new echarts.graphic.LinearGradient(0, 0, 0, 1, [
            { offset: 0, color: 'rgba(225, 29, 72, 0.22)' },
            { offset: 1, color: 'rgba(225, 29, 72, 0.01)' },
          ]),
        },
        itemStyle: { color: COLOR_ROSE },
        lineStyle: { width: 2 },
      },
    ],
  });
}

let resizeTimeout: ReturnType<typeof setTimeout> | null = null;
function handleResize() {
  if (resizeTimeout) clearTimeout(resizeTimeout);
  resizeTimeout = setTimeout(() => {
    qpsChart?.resize();
    tokenChart?.resize();
    latencyChart?.resize();
    errorChart?.resize();
  }, 100);
}

function isCanvasSupported(): boolean {
  try {
    if (typeof document === 'undefined') return false;
    const canvas = document.createElement('canvas');
    return !!canvas.getContext?.('2d');
  } catch {
    return false;
  }
}

onMounted(() => {
  if (!isCanvasSupported()) return;
  try {
    if (qpsChartRef.value) qpsChart = echarts.init(qpsChartRef.value);
    if (tokenChartRef.value) tokenChart = echarts.init(tokenChartRef.value);
    if (latencyChartRef.value) latencyChart = echarts.init(latencyChartRef.value);
    if (errorChartRef.value) errorChart = echarts.init(errorChartRef.value);

    updateCharts();
    window.addEventListener('resize', handleResize);
  } catch {
    // Headless test runner without 2d canvas
  }
});

onUnmounted(() => {
  window.removeEventListener('resize', handleResize);
  if (resizeTimeout) {
    clearTimeout(resizeTimeout);
    resizeTimeout = null;
  }
  try {
    qpsChart?.dispose();
    tokenChart?.dispose();
    latencyChart?.dispose();
    errorChart?.dispose();
  } catch {
    // Ignore dispose errors
  }
  qpsChart = null;
  tokenChart = null;
  latencyChart = null;
  errorChart = null;
});

watch(
  [() => props.history, () => props.historyData],
  () => {
    updateCharts();
  },
  { deep: true }
);
</script>

<template>
  <div class="mb-6">
    <!-- 图表全局头部与周期切换器 -->
    <div class="flex flex-wrap items-center justify-between gap-3 mb-4">
      <div>
        <div class="flex items-center gap-2">
          <h2 class="text-base font-bold text-slate-900 tracking-tight">趋势与指标分布</h2>
          <UiTooltip
            content="基于时序聚合引擎，支持 24小时、7天与30天多周期回溯分析，直观掌握并发洪峰、Token用量分布、延迟走势与异常波动。"
            wrap
          >
            <button
              type="button"
              aria-label="指标分布概览说明"
              class="text-slate-400 hover:text-slate-600 transition-colors cursor-help inline-flex items-center"
            >
              <Icons name="info" size="14" />
            </button>
          </UiTooltip>
        </div>
        <p class="text-[13px] text-slate-500">多周期并发、吞吐柱状分布与延迟波形追踪</p>
      </div>

      <!-- 24小时 / 7天 / 30天 Switch 选择器 -->
      <div class="segment-track inline-flex items-center">
        <button
          v-for="opt in rangeOptions"
          :key="opt.key"
          type="button"
          class="px-3.5 py-1.5 text-[13px] font-medium rounded-md transition-all duration-150 cursor-pointer"
          :class="[
            range === opt.key
              ? 'bg-white text-slate-950 font-semibold'
              : 'text-slate-600 hover:text-slate-900',
          ]"
          @click="emit('update:range', opt.key)"
        >
          {{ opt.label }}
        </button>
      </div>
    </div>

    <!-- 2x2 图表网格 -->
    <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
      <!-- 1. QPS 趋势 -->
      <div class="swiss-card p-5">
        <div class="flex items-center justify-between mb-2">
          <div class="flex items-center gap-1.5">
            <div class="text-[15px] font-semibold text-slate-900">QPS 并发洪峰</div>
            <UiTooltip
              content="展示选定周期（24小时/7天/30天）内，网关每秒处理的 API 请求频次（Queries Per Second），波峰反映系统流量高峰时刻。"
              wrap
            >
              <button
                type="button"
                aria-label="QPS 说明"
                class="text-slate-400 hover:text-slate-600 transition-colors cursor-help inline-flex items-center"
              >
                <Icons name="info" size="14" />
              </button>
            </UiTooltip>
          </div>
          <span class="text-[13px] text-slate-500 font-mono">req/s</span>
        </div>
        <div ref="qpsChartRef" class="w-full h-44" />
      </div>

      <!-- 2. Token 吞吐量 (全量输入/输出/缓存命中堆叠) -->
      <div class="swiss-card p-5">
        <div class="flex items-center justify-between mb-2">
          <div class="flex items-center gap-1.5">
            <div class="text-[15px] font-semibold text-slate-900">Token 吞吐量分布</div>
            <UiTooltip
              content="展示各时间切片内消耗的 Token 总量，按「输入 Token」、「输出 Token」和「缓存命中」三维度进行堆叠拆解，直观掌握算力与费用分布。"
              wrap
            >
              <button
                type="button"
                aria-label="Token 吞吐说明"
                class="text-slate-400 hover:text-slate-600 transition-colors cursor-help inline-flex items-center"
              >
                <Icons name="info" size="14" />
              </button>
            </UiTooltip>
          </div>
          <span class="text-[13px] text-slate-500 font-mono">tokens</span>
        </div>
        <div ref="tokenChartRef" class="w-full h-44" />
      </div>

      <!-- 3. TTFT 延迟与生成速率 -->
      <div class="swiss-card p-5">
        <div class="flex items-center justify-between mb-2">
          <div class="flex items-center gap-1.5">
            <div class="text-[15px] font-semibold text-slate-900">延迟与速率起伏</div>
            <UiTooltip
              content="展示网关端到端平均处理延迟（毫秒），平缓低位代表性能优异，尖峰通常代表上游排队或公网波动。"
              wrap
            >
              <button
                type="button"
                aria-label="延迟说明"
                class="text-slate-400 hover:text-slate-600 transition-colors cursor-help inline-flex items-center"
              >
                <Icons name="info" size="14" />
              </button>
            </UiTooltip>
          </div>
          <span class="text-[13px] text-slate-500 font-mono">ms</span>
        </div>
        <div ref="latencyChartRef" class="w-full h-44" />
      </div>

      <!-- 4. 故障率异常台阶 -->
      <div class="swiss-card p-5">
        <div class="flex items-center justify-between mb-2">
          <div class="flex items-center gap-1.5">
            <div class="text-[15px] font-semibold text-slate-900">故障率异常波动</div>
            <UiTooltip
              content="展示各周期切片内失败请求（上游 5xx、超时或鉴权失败）占总请求的比例。系统正常运转时应稳定在 0% 底部基准线。"
              wrap
            >
              <button
                type="button"
                aria-label="故障率说明"
                class="text-slate-400 hover:text-slate-600 transition-colors cursor-help inline-flex items-center"
              >
                <Icons name="info" size="14" />
              </button>
            </UiTooltip>
          </div>
          <span class="text-[13px] text-slate-500 font-mono">%</span>
        </div>
        <div ref="errorChartRef" class="w-full h-44" />
      </div>
    </div>
  </div>
</template>
