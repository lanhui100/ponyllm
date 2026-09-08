<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from 'vue';
import * as echarts from 'echarts/core';
import { LineChart, BarChart } from 'echarts/charts';
import { GridComponent, TooltipComponent, LegendComponent } from 'echarts/components';
import { CanvasRenderer } from 'echarts/renderers';
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

const tokenDimension = ref<'provider' | 'model'>('provider');

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
    return `${(d.getMonth() + 1).toString().padStart(2, '0')}/${d.getDate().toString().padStart(2, '0')} ${d.getHours().toString().padStart(2, '0')}:00`;
  }
  return `${d.getHours().toString().padStart(2, '0')}:${d.getMinutes().toString().padStart(2, '0')}`;
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
      axisLabel: { color: '#64748b', fontSize: 11 },
    },
    yAxis: {
      type: 'value',
      min: 0,
      splitLine: { lineStyle: { color: '#f1f5f9' } },
      axisLabel: { color: '#64748b', fontSize: 11 },
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

  // 2. Token 吞吐量 柱状分布图 (Bar Chart with Provider / Model dimension switch)
  if (useHistorical) {
    // Extract top keys for selected dimension
    const keysSet = new Set<string>();
    points.forEach((p) => {
      const dict = tokenDimension.value === 'provider' ? p.tokens_by_provider : p.tokens_by_model;
      if (dict) {
        Object.keys(dict).forEach((k) => keysSet.add(k));
      }
    });
    const seriesKeys = Array.from(keysSet).slice(0, 5); // top 5 series
    const palette = ['#0284c7', '#f97316', '#0d9488', '#8b5cf6', '#eab308'];

    const barSeries = seriesKeys.map((key, idx) => ({
      name: key,
      type: 'bar' as const,
      stack: 'total',
      barMaxWidth: 18,
      itemStyle: { color: palette[idx % palette.length], borderRadius: [2, 2, 0, 0] },
      data: points.map((p) => {
        const dict = tokenDimension.value === 'provider' ? p.tokens_by_provider : p.tokens_by_model;
        return dict?.[key] || 0;
      }),
    }));

    // Fallback single total bar if no sub-dimensions yet
    if (barSeries.length === 0) {
      barSeries.push({
        name: 'Token 总量',
        type: 'bar' as const,
        stack: 'total',
        barMaxWidth: 18,
        itemStyle: { color: COLOR_SLATE_CYAN, borderRadius: [3, 3, 0, 0] },
        data: points.map((p) => p.total_tokens),
      });
    }

    tokenChart?.setOption({
      tooltip: commonTooltip,
      legend: {
        show: seriesKeys.length > 1,
        top: 0,
        right: 0,
        textStyle: { fontSize: 11, color: '#64748b' },
        itemWidth: 10,
        itemHeight: 10,
      },
      grid: { ...commonGrid, top: seriesKeys.length > 1 ? 32 : 20 },
      xAxis: {
        type: 'category',
        data: timestamps,
        axisLine: { lineStyle: { color: '#e2e8f0' } },
        axisTick: { show: false },
        axisLabel: { color: '#64748b', fontSize: 11 },
      },
      yAxis: {
        type: 'value',
        min: 0,
        splitLine: { lineStyle: { color: '#f1f5f9' } },
        axisLabel: { color: '#64748b', fontSize: 11 },
      },
      series: barSeries,
    }, { notMerge: true });
  } else {
    // Realtime fallback single series
    const tokenData = props.history.map((h) => h.tokenThroughput);
    tokenChart?.setOption({
      tooltip: commonTooltip,
      grid: commonGrid,
      xAxis: {
        type: 'category',
        data: timestamps,
        axisLine: { lineStyle: { color: '#e2e8f0' } },
        axisTick: { show: false },
        axisLabel: { color: '#64748b', fontSize: 11 },
      },
      yAxis: {
        type: 'value',
        min: 0,
        splitLine: { lineStyle: { color: '#f1f5f9' } },
        axisLabel: { color: '#64748b', fontSize: 11 },
      },
      series: [
        {
          name: 'Tokens/s',
          type: 'bar',
          barMaxWidth: 16,
          itemStyle: { color: COLOR_SLATE_CYAN, borderRadius: [2, 2, 0, 0] },
          data: tokenData,
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
      axisLabel: { color: '#64748b', fontSize: 11 },
    },
    yAxis: {
      type: 'value',
      min: 0,
      splitLine: { lineStyle: { color: '#f1f5f9' } },
      axisLabel: { color: '#64748b', fontSize: 11 },
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
      axisLabel: { color: '#64748b', fontSize: 11 },
    },
    yAxis: {
      type: 'value',
      min: 0,
      max: 100,
      splitLine: { lineStyle: { color: '#f1f5f9' } },
      axisLabel: { color: '#64748b', fontSize: 11 },
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
  [() => props.history, () => props.historyData, () => tokenDimension.value],
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
        <h2 class="text-base font-bold text-slate-900 tracking-tight">趋势与指标分布</h2>
        <p class="text-xs text-slate-400">多周期并发、吞吐柱状分布与延迟波形追踪</p>
      </div>

      <!-- 24小时 / 7天 / 30天 Switch 选择器 -->
      <div class="segment-track inline-flex items-center">
        <button
          v-for="opt in rangeOptions"
          :key="opt.key"
          type="button"
          class="px-3 py-1.5 text-xs font-semibold rounded-md transition-all duration-150 cursor-pointer"
          :class="[
            range === opt.key
              ? 'bg-white text-orange-600 shadow-xs'
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
      <!-- 1. QPS 趋势 (折线面积图) -->
      <div class="borderless-card p-5">
        <div class="flex items-center justify-between mb-2">
          <div class="text-sm font-bold text-slate-800">QPS 并发洪峰 (折线面积图)</div>
          <span class="text-xs text-slate-400 font-mono">req/s</span>
        </div>
        <div ref="qpsChartRef" class="w-full h-44" />
      </div>

      <!-- 2. Token 吞吐量 (柱状分布图，带 Provider / Model 切换 switch) -->
      <div class="borderless-card p-5">
        <div class="flex items-center justify-between mb-2">
          <div class="text-sm font-bold text-slate-800">Token 吞吐量分布 (柱状分布图)</div>
          <!-- 维度切换 switch -->
          <div class="segment-track inline-flex items-center">
            <button
              type="button"
              class="px-2.5 py-1 text-[11px] font-semibold rounded transition-all cursor-pointer"
              :class="tokenDimension === 'provider' ? 'bg-white text-sky-700 shadow-xs' : 'text-slate-500'"
              @click="tokenDimension = 'provider'"
            >
              按 Provider
            </button>
            <button
              type="button"
              class="px-2.5 py-1 text-[11px] font-semibold rounded transition-all cursor-pointer"
              :class="tokenDimension === 'model' ? 'bg-white text-sky-700 shadow-xs' : 'text-slate-500'"
              @click="tokenDimension = 'model'"
            >
              按模型
            </button>
          </div>
        </div>
        <div ref="tokenChartRef" class="w-full h-44" />
      </div>

      <!-- 3. TTFT 延迟与生成速率 (波形图) -->
      <div class="borderless-card p-5">
        <div class="flex items-center justify-between mb-2">
          <div class="text-sm font-bold text-slate-800">延迟与速率起伏 (波形图)</div>
          <span class="text-xs text-slate-400 font-mono">ms</span>
        </div>
        <div ref="latencyChartRef" class="w-full h-44" />
      </div>

      <!-- 4. 故障率异常台阶 (微波阶梯图) -->
      <div class="borderless-card p-5">
        <div class="flex items-center justify-between mb-2">
          <div class="text-sm font-bold text-slate-800">故障率异常波动 (阶梯图)</div>
          <span class="text-xs text-slate-400 font-mono">%</span>
        </div>
        <div ref="errorChartRef" class="w-full h-44" />
      </div>
    </div>
  </div>
</template>
