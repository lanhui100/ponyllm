<script setup lang="ts">
import { ref } from 'vue';

defineProps<{
  content: string;
  position?: 'top' | 'bottom';
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
  <div class="relative inline-flex items-center" @mouseenter="show" @mouseleave="hide">
    <slot />
    <Transition name="fade-tooltip">
      <div
        v-if="visible && content"
        class="absolute z-50 px-2.5 py-1.5 text-xs font-normal text-white bg-slate-900/90 backdrop-blur-xs rounded-md shadow-md whitespace-nowrap pointer-events-none transition-all duration-150"
        :class="position === 'bottom' ? 'top-full mt-1.5 left-1/2 -translate-x-1/2' : 'bottom-full mb-1.5 left-1/2 -translate-x-1/2'"
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
