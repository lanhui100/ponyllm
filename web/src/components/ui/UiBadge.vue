<script setup lang="ts">
import { computed } from 'vue';

const props = withDefaults(
  defineProps<{
    variant?: 'default' | 'secondary' | 'success' | 'warning' | 'destructive' | 'outline' | 'purple';
    class?: string;
  }>(),
  {
    variant: 'default',
    class: '',
  }
);

const variantClass = computed(() => {
  switch (props.variant) {
    case 'secondary':
      return 'bg-slate-700 text-white';
    case 'success':
      return 'bg-emerald-600 text-white';
    case 'warning':
      return 'bg-amber-600 text-white';
    case 'destructive':
      return 'bg-rose-600 text-white';
    case 'purple':
      return 'bg-purple-600 text-white';
    case 'outline':
      return 'bg-slate-800 text-white border border-slate-600';
    case 'default':
    default:
      return 'bg-slate-800 text-white';
  }
});
</script>

<template>
  <span
    class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium tracking-normal"
    :class="[variantClass, props.class]"
  >
    <slot />
  </span>
</template>
