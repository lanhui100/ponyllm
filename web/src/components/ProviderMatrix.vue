<script setup lang="ts">
import type { ProviderFlowSnapshot } from '../types/telemetry';

defineProps<{
  providers: Record<string, ProviderFlowSnapshot> | undefined;
}>();
</script>

<template>
  <div class="provider-section">
    <div class="section-title">Provider 节点健康矩阵</div>
    <div v-if="!providers || Object.keys(providers).length === 0" class="empty-hint">
      暂无活跃 Provider 节点数据
    </div>
    <div v-else class="table-wrap">
      <table class="matrix-table">
        <thead>
          <tr>
            <th>Provider</th>
            <th>状态</th>
            <th>流调用数</th>
            <th>平均 TTFT</th>
            <th>平均 TPS</th>
            <th>错误数</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="(p, name) in providers" :key="name">
            <td class="font-medium">{{ name }}</td>
            <td>
              <span class="badge" :class="p.status || 'healthy'">
                {{ (p.status || 'healthy').toUpperCase() }}
              </span>
            </td>
            <td>{{ p.stream_count ?? 0 }}</td>
            <td>{{ p.avg_ttft_ms !== undefined && p.avg_ttft_ms !== null ? `${p.avg_ttft_ms.toFixed(1)} ms` : '--' }}</td>
            <td>{{ p.avg_tps !== undefined && p.avg_tps !== null ? `${p.avg_tps.toFixed(1)} tok/s` : '--' }}</td>
            <td :class="{ 'text-error': (p.error_count ?? 0) > 0 }">{{ p.error_count ?? 0 }}</td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>

<style scoped>
.provider-section {
  background: #ffffff;
  border: 1px solid #e2e8f0;
  border-radius: 8px;
  padding: 16px 20px;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.04);
}

.section-title {
  font-size: 15px;
  font-weight: 600;
  color: #1e293b;
  margin-bottom: 12px;
}

.empty-hint {
  font-size: 13px;
  color: #94a3b8;
  padding: 24px 0;
  text-align: center;
}

.table-wrap {
  overflow-x: auto;
}

.matrix-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 13px;
}

.matrix-table th {
  text-align: left;
  padding: 10px 12px;
  color: #64748b;
  font-weight: 500;
  border-bottom: 1px solid #e2e8f0;
}

.matrix-table td {
  padding: 12px;
  border-bottom: 1px solid #f1f5f9;
  color: #334155;
}

.font-medium {
  font-weight: 500;
  color: #0f172a;
}

.badge {
  font-size: 11px;
  padding: 2px 8px;
  border-radius: 10px;
  font-weight: 500;
}

.badge.healthy {
  background: #dcfce7;
  color: #15803d;
}

.badge.degraded {
  background: #fef9c3;
  color: #a16207;
}

.badge.unhealthy {
  background: #fee2e2;
  color: #b91c1c;
}

.text-error {
  color: #ef4444;
  font-weight: 500;
}
</style>
