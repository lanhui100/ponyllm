<script setup lang="ts">
defineProps<{
  open: boolean;
}>();
</script>

<template>
  <Transition
    enter-active-class="transition-all duration-250 ease-out overflow-hidden"
    leave-active-class="transition-all duration-200 ease-in overflow-hidden"
    enter-from-class="opacity-0 max-h-0 -translate-y-1"
    enter-to-class="opacity-100 max-h-[1600px] translate-y-0"
    leave-from-class="opacity-100 max-h-[1600px] translate-y-0"
    leave-to-class="opacity-0 max-h-0 -translate-y-1"
  >
    <div v-show="open">
      <slot />
    </div>
  </Transition>
</template>
