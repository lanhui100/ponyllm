<script setup lang="ts">
defineProps<{
  class?: string;
}>();
</script>

<template>
  <div
    class="bg-white rounded-xl shadow-xs transition-shadow duration-200"
    :class="$props.class"
  >
    <slot />
  </div>
</template>
