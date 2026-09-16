<script setup lang="ts">
/**
 * DashboardSkeleton —— 仪表盘首屏骨架，与真实布局同构等高：
 * 状态横幅条 → 5 张指标卡 → 趋势 2×2 图表区 → 提供商表格。
 */
import UiSkeleton from './ui/UiSkeleton.vue';
</script>

<template>
  <div
    role="status"
    aria-label="仪表盘数据加载中"
    data-testid="dashboard-skeleton"
    class="space-y-4"
  >
    <!-- 状态横幅条 -->
    <div class="swiss-card flex items-center gap-4 px-5 py-3.5">
      <UiSkeleton variant="circle" width="10px" height="10px" />
      <UiSkeleton width="64px" height="14px" />
      <div class="flex items-center gap-[2px]" aria-hidden="true">
        <UiSkeleton v-for="i in 24" :key="i" variant="rect" width="4px" height="18px" radius="1px" />
      </div>
      <UiSkeleton width="80px" height="14px" class="ml-auto" />
    </div>

    <!-- 指标卡：5 列 -->
    <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-5 gap-4" aria-hidden="true">
      <div v-for="i in 5" :key="i" class="swiss-card p-5 space-y-2.5">
        <UiSkeleton width="72px" height="13px" />
        <UiSkeleton width="60%" height="30px" radius="6px" />
        <UiSkeleton width="85%" height="12px" />
      </div>
    </div>

    <!-- 趋势图表：标题行 + 2×2 -->
    <div aria-hidden="true">
      <div class="flex items-center justify-between mb-4">
        <div class="space-y-2">
          <UiSkeleton width="140px" height="16px" />
          <UiSkeleton width="220px" height="12px" />
        </div>
        <UiSkeleton variant="rect" width="180px" height="30px" radius="8px" />
      </div>
      <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
        <div v-for="i in 4" :key="i" class="swiss-card p-5 space-y-3">
          <div class="flex items-center justify-between">
            <UiSkeleton width="120px" height="15px" />
            <UiSkeleton width="44px" height="12px" />
          </div>
          <UiSkeleton variant="rect" width="100%" height="176px" radius="8px" />
        </div>
      </div>
    </div>

    <!-- 提供商表格 -->
    <div class="swiss-card p-6" aria-hidden="true">
      <div class="flex items-center gap-2.5 pb-4">
        <UiSkeleton variant="rect" width="32px" height="32px" radius="8px" />
        <div class="space-y-1.5">
          <UiSkeleton width="110px" height="15px" />
          <UiSkeleton width="200px" height="12px" />
        </div>
      </div>
      <div class="space-y-3">
        <div v-for="i in 4" :key="i" class="flex items-center gap-4">
          <UiSkeleton width="90px" height="22px" radius="6px" />
          <div class="flex items-center gap-[2px]">
            <UiSkeleton v-for="j in 12" :key="j" variant="rect" width="4px" height="16px" radius="1px" />
          </div>
          <UiSkeleton width="56px" height="14px" />
          <UiSkeleton width="56px" height="14px" />
          <UiSkeleton width="48px" height="14px" class="ml-auto" />
        </div>
      </div>
    </div>
  </div>
</template>
