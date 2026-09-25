<script setup lang="ts">
import { ref, computed, nextTick } from 'vue';

const props = defineProps<{
  content?: string;
  position?: 'top' | 'bottom';
  wrap?: boolean;
}>();

const visible = ref(false);
const triggerRef = ref<HTMLElement | null>(null);
const tooltipRef = ref<HTMLElement | null>(null);
// 锚点在视口坐标系中的位置（用于溢出翻转判断），tooltip 自身尺寸（用于左右钳位）
const anchorViewport = ref({ top: 0, bottom: 0, centerX: 0 });
const tooltipSize = ref({ width: 0, height: 0 });
const flippedToBottom = ref(false);
let timer: ReturnType<typeof setTimeout> | null = null;

// 距视口边缘的安全边距，避免 tooltip 贴边
const VIEWPORT_MARGIN = 8;

function updatePosition() {
  if (!triggerRef.value) return;
  const rect = triggerRef.value.getBoundingClientRect();
  anchorViewport.value = {
    top: rect.top,
    bottom: rect.bottom,
    centerX: rect.left + rect.width / 2,
  };
  flippedToBottom.value = false;
  tooltipSize.value = { width: 0, height: 0 };
}

function measureTooltip() {
  const el = tooltipRef.value;
  if (!el) return;
  const rect = el.getBoundingClientRect();
  // happy-dom 下拿不到布局尺寸时保持 0，走居中兜底分支
  if (rect.width > 0) {
    tooltipSize.value = { width: rect.width, height: rect.height };
  }
  // 顶部空间不足时翻转到底部，避免顶部被裁剪
  const isBottom = props.position === 'bottom';
  if (!isBottom && rect.height > 0 && anchorViewport.value.top - 6 - rect.height < VIEWPORT_MARGIN) {
    flippedToBottom.value = true;
  }
}

function show() {
  timer = setTimeout(() => {
    updatePosition();
    visible.value = true;
    // 等待 Teleport 挂载后测量真实尺寸，再做左右钳位与顶部翻转
    nextTick(() => measureTooltip());
  }, 120);
}

function hide() {
  if (timer) clearTimeout(timer);
  visible.value = false;
}

const tooltipStyle = computed(() => {
  const viewportWidth = typeof window !== 'undefined' ? window.innerWidth || 0 : 0;
  const showBelow = props.position === 'bottom' || flippedToBottom.value;
  const topViewport = showBelow ? anchorViewport.value.bottom + 6 : anchorViewport.value.top - 6;
  let leftViewport = anchorViewport.value.centerX;
  let translateX = '-50%';
  const halfWidth = tooltipSize.value.width > 0 ? tooltipSize.value.width / 2 : 0;
  if (halfWidth > 0 && viewportWidth > 0) {
    // 左右钳位：tooltip 半宽超出视口时，改为按边对齐
    if (leftViewport - halfWidth < VIEWPORT_MARGIN) {
      leftViewport = VIEWPORT_MARGIN;
      translateX = '0';
    } else if (leftViewport + halfWidth > viewportWidth - VIEWPORT_MARGIN) {
      leftViewport = viewportWidth - VIEWPORT_MARGIN;
      translateX = '-100%';
    }
  }
  return {
    top: `${topViewport}px`,
    left: `${leftViewport}px`,
    transform: showBelow ? `translate(${translateX}, 0)` : `translate(${translateX}, -100%)`,
    maxWidth: 'calc(100vw - 16px)',
  };
});
</script>

<template>
  <div
    ref="triggerRef"
    class="relative inline-flex items-center"
    @mouseenter="show"
    @mouseleave="hide"
    @focusin="show"
    @focusout="hide"
  >
    <slot />
    <Teleport to="body">
      <Transition name="fade-tooltip">
        <div
          v-if="visible && (content || $slots.content)"
          ref="tooltipRef"
          role="tooltip"
          class="fixed z-[9999] px-3 py-2 text-[13px] font-medium text-white bg-slate-900/95 backdrop-blur-xs rounded-lg shadow-2xl pointer-events-none border border-slate-700/60 whitespace-pre-line"
          :class="[
            wrap ? 'w-72 max-w-sm whitespace-pre-line break-words leading-relaxed text-left max-h-72 overflow-y-auto' : '',
          ]"
          :style="tooltipStyle"
          data-testid="ui-tooltip"
        >
          <slot name="content">{{ content }}</slot>
        </div>
      </Transition>
    </Teleport>
  </div>
</template>

<style scoped>
.fade-tooltip-enter-active,
.fade-tooltip-leave-active {
  transition: opacity 0.15s ease;
}
.fade-tooltip-enter-from,
.fade-tooltip-leave-to {
  opacity: 0;
}
.fade-tooltip-enter-to,
.fade-tooltip-leave-from {
  opacity: 1;
}
</style>
