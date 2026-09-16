<script setup lang="ts">
/**
 * UiSkeleton —— 基础 shimmer 占位块。
 * 自身 aria-hidden，由父级骨架容器统一暴露 role="status" + aria-label。
 */
withDefaults(
  defineProps<{
    /** 预设形状：文本行 / 圆形 / 矩形 */
    variant?: 'text' | 'circle' | 'rect';
    /** 宽度（任意 CSS 值） */
    width?: string;
    /** 高度（任意 CSS 值） */
    height?: string;
    /** 圆角（任意 CSS 值，variant=text 时默认 pill） */
    radius?: string;
    /** 行内样式补充 */
    class?: string;
  }>(),
  {
    variant: 'text',
    width: '100%',
    height: undefined,
    radius: undefined,
    class: '',
  }
);
</script>

<template>
  <span
    aria-hidden="true"
    class="skeleton-block"
    :class="[
      variant === 'circle' ? 'skeleton-circle' : variant === 'rect' ? 'skeleton-rect' : 'skeleton-text',
      $props.class,
    ]"
    :style="{
      width,
      ...(height ? { height } : {}),
      ...(radius ? { borderRadius: radius } : {}),
    }"
  />
</template>

<style scoped>
.skeleton-text {
  height: 14px;
  border-radius: 9999px;
}
.skeleton-circle {
  border-radius: 9999px;
  aspect-ratio: 1 / 1;
}
.skeleton-rect {
  border-radius: 8px;
}
</style>
