<script setup lang="ts">
defineProps<{
  class?: string;
}>();
</script>

<template>
  <div
    class="bg-white/45 backdrop-blur-xs rounded-xl transition-shadow duration-200 border border-white/40 shadow-xs"
    :class="$props.class"
  >
    <slot />
  </div>
</template>
