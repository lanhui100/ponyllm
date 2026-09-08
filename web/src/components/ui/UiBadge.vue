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
      return 'bg-emerald-50 text-emerald-700';
    case 'warning':
      return 'bg-amber-50 text-amber-700';
    case 'destructive':
      return 'bg-rose-50 text-rose-700';
    case 'purple':
      return 'bg-purple-50 text-purple-700';
    case 'outline':
      return 'bg-transparent text-slate-600 border border-slate-200';
    case 'default':
    default:
      return 'bg-blue-50 text-blue-700';
  }
});
</script>

<template>
  <span
    class="inline-flex items-center px-2 py-0.5 rounded-full text-2xs font-medium tracking-wide"
    :class="[variantClass, props.class]"
  >
    <slot />
  </span>
</template>
