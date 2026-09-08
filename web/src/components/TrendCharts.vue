<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch } from 'vue';
import * as echarts from 'echarts/core';
import { LineChart } from 'echarts/charts';
import { GridComponent, TooltipComponent } from 'echarts/components';
import { CanvasRenderer } from 'echarts/renderers';
import type { TelemetryPoint } from '../composables/useTelemetry';

echarts.use([LineChart, GridComponent, TooltipComponent, CanvasRenderer]);

const props = defineProps<{
  history: TelemetryPoint[];
}>();

const qpsChartRef = ref<HTMLDivElement | null>(null);
const tokenChartRef = ref<HTMLDivElement | null>(null);
const latencyChartRef = ref<HTMLDivElement | null>(null);
const errorChartRef = ref<HTMLDivElement | null>(null);

let qpsChart: echarts.ECharts | null = null;
let tokenChart: echarts.ECharts | null = null;
let latencyChart: echarts.ECharts | null = null;
let errorChart: echarts.ECharts | null = null;

function formatTime(ts: number): string {
  const d = new Date(ts);
  return `${d.getMinutes().toString().padStart(2, '0')}:${d.getSeconds().toString().padStart(2, '0')}`;
}

const commonTooltip = {
  trigger: 'axis' as const,
  backgroundColor: 'rgba(15, 23, 42, 0.88)',
  borderColor: 'transparent',
  borderWidth: 0,
  padding: [6, 10],
  textStyle: {
    color: '#ffffff',
    fontSize: 12,
  },
};

const commonGrid = { top: 16, right: 12, bottom: 20, left: 36 };

function updateCharts() {
  const timestamps = props.history.map((h) => formatTime(h.timestamp));
  const qpsData = props.history.map((h) => h.qps);
  const tokenData = props.history.map((h) => h.tokenThroughput);
  const latencyData = props.history.map((h) => h.latencyMs);
  const errorData = props.history.map((h) => h.errorRate);

  qpsChart?.setOption({
    tooltip: commonTooltip,
    grid: commonGrid,
    xAxis: {
      type: 'category',
      data: timestamps,
      boundaryGap: false,
      axisLine: { lineStyle: { color: '#e2e8f0' } },
      axisTick: { show: false },
      axisLabel: { color: '#94a3b8', fontSize: 11 },
    },
    yAxis: {
      type: 'value',
      min: 0,
      splitLine: { lineStyle: { color: '#f8fafc' } },
      axisLabel: { color: '#94a3b8', fontSize: 11 },
    },
    series: [
      {
        name: 'QPS',
        type: 'line',
        smooth: true,
        showSymbol: false,
        data: qpsData,
        areaStyle: {
          color: new echarts.graphic.LinearGradient(0, 0, 0, 1, [
            { offset: 0, color: 'rgba(59, 130, 246, 0.25)' },
            { offset: 1, color: 'rgba(59, 130, 246, 0.01)' },
          ]),
        },
        itemStyle: { color: '#3b82f6' },
        lineStyle: { width: 2 },
      },
    ],
  });

  tokenChart?.setOption({
    tooltip: commonTooltip,
    grid: { ...commonGrid, left: 45 },
    xAxis: {
      type: 'category',
      data: timestamps,
      boundaryGap: false,
      axisLine: { lineStyle: { color: '#e2e8f0' } },
      axisTick: { show: false },
      axisLabel: { color: '#94a3b8', fontSize: 11 },
    },
    yAxis: {
      type: 'value',
      min: 0,
      splitLine: { lineStyle: { color: '#f8fafc' } },
      axisLabel: { color: '#94a3b8', fontSize: 11 },
    },
    series: [
      {
        name: 'Tokens/s',
        type: 'line',
        smooth: true,
        showSymbol: false,
        data: tokenData,
        areaStyle: {
          color: new echarts.graphic.LinearGradient(0, 0, 0, 1, [
            { offset: 0, color: 'rgba(16, 185, 129, 0.25)' },
            { offset: 1, color: 'rgba(16, 185, 129, 0.01)' },
          ]),
        },
        itemStyle: { color: '#10b981' },
        lineStyle: { width: 2 },
      },
    ],
  });

  latencyChart?.setOption({
    tooltip: commonTooltip,
    grid: { ...commonGrid, left: 45 },
    xAxis: {
      type: 'category',
      data: timestamps,
      boundaryGap: false,
      axisLine: { lineStyle: { color: '#e2e8f0' } },
      axisTick: { show: false },
      axisLabel: { color: '#94a3b8', fontSize: 11 },
    },
    yAxis: {
      type: 'value',
      min: 0,
      splitLine: { lineStyle: { color: '#f8fafc' } },
      axisLabel: { color: '#94a3b8', fontSize: 11 },
    },
    series: [
      {
        name: '延迟 (ms)',
        type: 'line',
        smooth: true,
        showSymbol: false,
        data: latencyData,
        areaStyle: {
          color: new echarts.graphic.LinearGradient(0, 0, 0, 1, [
            { offset: 0, color: 'rgba(245, 158, 11, 0.25)' },
            { offset: 1, color: 'rgba(245, 158, 11, 0.01)' },
          ]),
        },
        itemStyle: { color: '#f59e0b' },
        lineStyle: { width: 2 },
      },
    ],
  });

  errorChart?.setOption({
    tooltip: commonTooltip,
    grid: commonGrid,
    xAxis: {
      type: 'category',
      data: timestamps,
      boundaryGap: false,
      axisLine: { lineStyle: { color: '#e2e8f0' } },
      axisTick: { show: false },
      axisLabel: { color: '#94a3b8', fontSize: 11 },
    },
    yAxis: {
      type: 'value',
      min: 0,
      max: 100,
      splitLine: { lineStyle: { color: '#f8fafc' } },
      axisLabel: { color: '#94a3b8', fontSize: 11 },
    },
    series: [
      {
        name: '故障率 (%)',
        type: 'line',
        smooth: true,
        showSymbol: false,
        data: errorData,
        areaStyle: {
          color: new echarts.graphic.LinearGradient(0, 0, 0, 1, [
            { offset: 0, color: 'rgba(239, 68, 68, 0.25)' },
            { offset: 1, color: 'rgba(239, 68, 68, 0.01)' },
          ]),
        },
        itemStyle: { color: '#ef4444' },
        lineStyle: { width: 2 },
      },
    ],
  });
}

function handleResize() {
  qpsChart?.resize();
  tokenChart?.resize();
  latencyChart?.resize();
  errorChart?.resize();
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
    // Headless test runner without 2d canvas context
  }
});

onUnmounted(() => {
  window.removeEventListener('resize', handleResize);
  try {
    qpsChart?.dispose();
    tokenChart?.dispose();
    latencyChart?.dispose();
    errorChart?.dispose();
  } catch {
    // Ignore dispose errors
  }
});

watch(
  () => props.history,
  () => {
    updateCharts();
  },
  { deep: true }
);
</script>

<template>
  <div class="grid grid-cols-1 md:grid-cols-2 gap-4 mb-6">
    <div class="bg-white rounded-xl shadow-xs p-4">
      <div class="text-xs font-semibold text-slate-700 mb-2">QPS 趋势 (30s)</div>
      <div ref="qpsChartRef" class="w-full h-44" />
    </div>

    <div class="bg-white rounded-xl shadow-xs p-4">
      <div class="text-xs font-semibold text-slate-700 mb-2">Token 吞吐量趋势 (tok/s)</div>
      <div ref="tokenChartRef" class="w-full h-44" />
    </div>

    <div class="bg-white rounded-xl shadow-xs p-4">
      <div class="text-xs font-semibold text-slate-700 mb-2">TTFT 延迟趋势 (ms)</div>
      <div ref="latencyChartRef" class="w-full h-44" />
    </div>

    <div class="bg-white rounded-xl shadow-xs p-4">
      <div class="text-xs font-semibold text-slate-700 mb-2">故障率趋势 (%)</div>
      <div ref="errorChartRef" class="w-full h-44" />
    </div>
  </div>
</template>
