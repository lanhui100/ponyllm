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

function updateCharts() {
  const timestamps = props.history.map((h) => formatTime(h.timestamp));
  const qpsData = props.history.map((h) => h.qps);
  const tokenData = props.history.map((h) => h.tokenThroughput);
  const latencyData = props.history.map((h) => h.latencyMs);
  const errorData = props.history.map((h) => h.errorRate);

  qpsChart?.setOption({
    title: { show: false },
    tooltip: { trigger: 'axis' },
    grid: { top: 20, right: 20, bottom: 25, left: 40 },
    xAxis: { type: 'category', data: timestamps, boundaryGap: false },
    yAxis: { type: 'value', min: 0 },
    series: [
      {
        name: 'QPS',
        type: 'line',
        smooth: true,
        data: qpsData,
        areaStyle: { opacity: 0.15 },
        itemStyle: { color: '#3b82f6' },
      },
    ],
  });

  tokenChart?.setOption({
    title: { show: false },
    tooltip: { trigger: 'axis' },
    grid: { top: 20, right: 20, bottom: 25, left: 50 },
    xAxis: { type: 'category', data: timestamps, boundaryGap: false },
    yAxis: { type: 'value', min: 0 },
    series: [
      {
        name: 'Tokens/s',
        type: 'line',
        smooth: true,
        data: tokenData,
        areaStyle: { opacity: 0.15 },
        itemStyle: { color: '#10b981' },
      },
    ],
  });

  latencyChart?.setOption({
    title: { show: false },
    tooltip: { trigger: 'axis' },
    grid: { top: 20, right: 20, bottom: 25, left: 50 },
    xAxis: { type: 'category', data: timestamps, boundaryGap: false },
    yAxis: { type: 'value', min: 0 },
    series: [
      {
        name: '延迟 (ms)',
        type: 'line',
        smooth: true,
        data: latencyData,
        areaStyle: { opacity: 0.15 },
        itemStyle: { color: '#f59e0b' },
      },
    ],
  });

  errorChart?.setOption({
    title: { show: false },
    tooltip: { trigger: 'axis' },
    grid: { top: 20, right: 20, bottom: 25, left: 40 },
    xAxis: { type: 'category', data: timestamps, boundaryGap: false },
    yAxis: { type: 'value', min: 0, max: 100 },
    series: [
      {
        name: '故障率 (%)',
        type: 'line',
        smooth: true,
        data: errorData,
        areaStyle: { opacity: 0.15 },
        itemStyle: { color: '#ef4444' },
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

watch(() => props.history, () => {
  updateCharts();
}, { deep: true });
</script>

<template>
  <div class="charts-grid">
    <div class="chart-box">
      <div class="chart-header">QPS 趋势 (30s)</div>
      <div ref="qpsChartRef" class="chart-container" />
    </div>

    <div class="chart-box">
      <div class="chart-header">Token 吞吐量趋势 (tok/s)</div>
      <div ref="tokenChartRef" class="chart-container" />
    </div>

    <div class="chart-box">
      <div class="chart-header">TTFT 延迟趋势 (ms)</div>
      <div ref="latencyChartRef" class="chart-container" />
    </div>

    <div class="chart-box">
      <div class="chart-header">故障率趋势 (%)</div>
      <div ref="errorChartRef" class="chart-container" />
    </div>
  </div>
</template>

<style scoped>
.charts-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(360px, 1fr));
  gap: 16px;
  margin-bottom: 24px;
}

.chart-box {
  background: #ffffff;
  border: 1px solid #e2e8f0;
  border-radius: 8px;
  padding: 16px;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.04);
}

.chart-header {
  font-size: 14px;
  font-weight: 500;
  color: #334155;
  margin-bottom: 12px;
}

.chart-container {
  width: 100%;
  height: 200px;
}
</style>
