<script setup lang="ts">
import { ref, computed } from 'vue';

const props = defineProps<{
  content?: string;
  position?: 'top' | 'bottom';
  wrap?: boolean;
}>();

const visible = ref(false);
const triggerRef = ref<HTMLElement | null>(null);
const coords = ref({ top: 0, left: 0 });
let timer: ReturnType<typeof setTimeout> | null = null;

function updatePosition() {
  if (!triggerRef.value) return;
  const rect = triggerRef.value.getBoundingClientRect();
  const isBottom = props.position === 'bottom';
  const scrollY = typeof window !== 'undefined' ? window.scrollY || 0 : 0;
  const scrollX = typeof window !== 'undefined' ? window.scrollX || 0 : 0;
  coords.value = {
    top: isBottom ? rect.bottom + scrollY + 6 : rect.top + scrollY - 6,
    left: rect.left + scrollX + rect.width / 2,
  };
}

function show() {
  timer = setTimeout(() => {
    updatePosition();
    visible.value = true;
  }, 120);
}

function hide() {
  if (timer) clearTimeout(timer);
  visible.value = false;
}

const tooltipStyle = computed(() => {
  const scrollY = typeof window !== 'undefined' ? window.scrollY || 0 : 0;
  const scrollX = typeof window !== 'undefined' ? window.scrollX || 0 : 0;
  const top = coords.value.top - scrollY;
  const left = coords.value.left - scrollX;
  return {
    top: `${top}px`,
    left: `${left}px`,
    transform: props.position === 'bottom' ? 'translate(-50%, 0)' : 'translate(-50%, -100%)',
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
          role="tooltip"
          class="fixed z-[9999] px-3 py-2 text-[13px] font-medium text-white bg-slate-900/95 backdrop-blur-xs rounded-lg shadow-2xl pointer-events-none border border-slate-700/60"
          :class="[
            wrap ? 'w-64 max-w-xs whitespace-normal leading-relaxed text-left' : 'whitespace-nowrap',
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
