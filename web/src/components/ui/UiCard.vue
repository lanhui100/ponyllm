<script setup lang="ts">
defineProps<{
  class?: string;
}>();
</script>

<template>
  <div
    class="bg-white rounded-xl border border-slate-200/90 shadow-2xs transition-shadow duration-200"
    :class="$props.class"
  >
    <slot />
  </div>
</template>
