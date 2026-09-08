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
      return 'bg-slate-100 text-slate-600';
    case 'success':
      return 'bg-teal-50/90 text-teal-800';
    case 'warning':
      return 'bg-amber-50/90 text-amber-800';
    case 'destructive':
      return 'bg-rose-50/90 text-rose-700';
    case 'purple':
      return 'bg-purple-50/90 text-purple-700';
    case 'outline':
      return 'bg-transparent text-slate-600 border border-slate-200';
    case 'default':
    default:
      return 'bg-orange-50/90 text-orange-700';
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
