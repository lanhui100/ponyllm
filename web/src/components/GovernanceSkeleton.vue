<script setup lang="ts">
/**
 * GovernanceSkeleton —— 模型管理首屏骨架：
 * 分类标签行 → 服务商卡（头部 + 模型行 + 密钥行）× 2。
 */
import UiSkeleton from './ui/UiSkeleton.vue';
</script>

<template>
  <div
    role="status"
    aria-label="模型配置加载中"
    data-testid="governance-skeleton"
    class="space-y-4"
  >
    <!-- 分类标签行 -->
    <div class="flex items-center gap-2 border-b border-white/50 pb-2" aria-hidden="true">
      <UiSkeleton variant="rect" width="132px" height="36px" radius="8px" />
      <UiSkeleton variant="rect" width="92px" height="36px" radius="8px" />
    </div>

    <!-- 服务商卡 × 2 -->
    <div v-for="card in 2" :key="card" class="swiss-card" aria-hidden="true">
      <!-- 卡片头部 -->
      <div class="p-5 flex items-center gap-3.5">
        <UiSkeleton variant="rect" width="42px" height="42px" radius="12px" />
        <div class="space-y-2 min-w-0 flex-1">
          <div class="flex items-center gap-2.5">
            <UiSkeleton width="110px" height="18px" />
            <UiSkeleton width="72px" height="20px" radius="6px" />
          </div>
          <UiSkeleton width="180px" height="13px" />
        </div>
        <div class="hidden sm:flex items-center gap-2 shrink-0">
          <UiSkeleton width="72px" height="22px" radius="6px" />
          <UiSkeleton width="96px" height="22px" radius="6px" />
        </div>
      </div>

      <!-- 模型行 × 2 -->
      <div class="px-5 pb-2 space-y-2.5">
        <div v-for="row in 2" :key="`m-${row}`" class="flex items-center gap-3 rounded-lg bg-white/40 px-3.5 py-2.5">
          <UiSkeleton variant="circle" width="28px" height="28px" />
          <UiSkeleton width="140px" height="14px" />
          <UiSkeleton width="52px" height="20px" radius="6px" />
          <UiSkeleton width="64px" height="20px" radius="6px" class="ml-auto" />
        </div>
      </div>

      <!-- 密钥行 × 2 -->
      <div class="px-5 pb-5 space-y-2.5">
        <div v-for="row in 2" :key="`k-${row}`" class="flex items-center gap-3 rounded-lg bg-white/40 px-3.5 py-2.5">
          <UiSkeleton width="120px" height="13px" />
          <UiSkeleton width="88px" height="20px" radius="6px" />
          <UiSkeleton variant="circle" width="28px" height="28px" class="ml-auto" />
          <UiSkeleton variant="circle" width="28px" height="28px" />
        </div>
      </div>
    </div>
  </div>
</template>
