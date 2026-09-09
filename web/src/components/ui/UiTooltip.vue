<script setup lang="ts">
import { ref } from 'vue';

defineProps<{
  content: string;
  position?: 'top' | 'bottom';
  wrap?: boolean;
}>();

const visible = ref(false);
let timer: ReturnType<typeof setTimeout> | null = null;

function show() {
  timer = setTimeout(() => {
    visible.value = true;
  }, 120);
}

function hide() {
  if (timer) clearTimeout(timer);
  visible.value = false;
}
</script>

<template>
  <div
    class="relative inline-flex items-center"
    @mouseenter="show"
    @mouseleave="hide"
    @focusin="show"
    @focusout="hide"
  >
    <slot />
    <Transition name="fade-tooltip">
      <div
        v-if="visible && content"
        role="tooltip"
        class="absolute z-50 px-3 py-2 text-[13px] font-medium text-white bg-slate-900/95 backdrop-blur-xs rounded-lg shadow-xl pointer-events-none transition-all duration-150 border border-slate-700/60"
        :class="[
          position === 'bottom' ? 'top-full mt-1.5 left-1/2 -translate-x-1/2' : 'bottom-full mb-1.5 left-1/2 -translate-x-1/2',
          wrap ? 'w-64 max-w-xs whitespace-normal leading-relaxed text-left' : 'whitespace-nowrap',
        ]"
        data-testid="ui-tooltip"
      >
        {{ content }}
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.fade-tooltip-enter-active,
.fade-tooltip-leave-active {
  transition: opacity 0.15s ease, transform 0.15s ease;
}
.fade-tooltip-enter-from,
.fade-tooltip-leave-to {
  opacity: 0;
  transform: translate(-50%, 3px);
}
.fade-tooltip-enter-to,
.fade-tooltip-leave-from {
  opacity: 1;
  transform: translate(-50%, 0);
}
</style>
