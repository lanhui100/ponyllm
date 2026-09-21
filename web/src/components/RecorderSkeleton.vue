<script setup lang="ts">
/**
 * RecorderSkeleton —— 轨迹页首屏骨架：表头 → 8 行轨迹行 → 底部分页条。
 * 过滤条与标题保持常显，仅替换数据区。
 */
import UiSkeleton from './ui/UiSkeleton.vue';
</script>

<template>
  <div
    role="status"
    aria-label="轨迹数据加载中"
    data-testid="recorder-skeleton"
    class="swiss-card flex-1 min-h-0 flex flex-col overflow-hidden border border-white/40 shadow-xs"
  >
    <!-- 表头 -->
    <div
      class="flex items-center gap-4 px-8 py-3.5 bg-slate-100/90 border-b border-slate-200/80"
      aria-hidden="true"
    >
      <UiSkeleton width="56px" height="12px" />
      <UiSkeleton width="56px" height="12px" />
      <UiSkeleton width="160px" height="12px" />
      <UiSkeleton width="72px" height="12px" />
      <UiSkeleton width="88px" height="12px" />
      <UiSkeleton width="96px" height="12px" />
      <UiSkeleton width="200px" height="12px" class="hidden lg:inline-block" />
      <UiSkeleton width="140px" height="12px" class="ml-auto" />
    </div>

    <!-- 数据行 × 8 -->
    <div class="flex-1 px-8 py-2 space-y-1" aria-hidden="true">
      <div
        v-for="i in 8"
        :key="i"
        class="flex items-center gap-4 py-3 border-b border-slate-100/80 last:border-0"
      >
        <UiSkeleton width="44px" height="20px" radius="4px" />
        <UiSkeleton width="64px" height="12px" />
        <UiSkeleton width="180px" height="12px" />
        <UiSkeleton width="72px" height="20px" radius="4px" />
        <UiSkeleton width="96px" height="12px" />
        <UiSkeleton width="88px" height="12px" />
        <UiSkeleton width="220px" height="12px" class="hidden lg:inline-block" />
        <UiSkeleton width="140px" height="12px" class="ml-auto" />
      </div>
    </div>

    <!-- 底部分页条 -->
    <div
      class="shrink-0 flex items-center justify-between gap-3 px-8 py-3 bg-white/50 border-t border-slate-200/60"
      aria-hidden="true"
    >
      <UiSkeleton width="150px" height="12px" />
      <div class="flex items-center gap-1.5 ml-auto">
        <UiSkeleton variant="rect" width="76px" height="28px" radius="8px" />
        <UiSkeleton width="48px" height="12px" />
        <UiSkeleton variant="rect" width="76px" height="28px" radius="8px" />
      </div>
    </div>
  </div>
</template>
