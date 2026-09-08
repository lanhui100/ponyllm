<script setup lang="ts">
import { computed } from 'vue';

const props = withDefaults(
  defineProps<{
    variant?: 'default' | 'outline' | 'ghost' | 'destructive' | 'secondary';
    size?: 'default' | 'sm' | 'lg' | 'icon';
    disabled?: boolean;
    type?: 'button' | 'submit' | 'reset';
    class?: string;
  }>(),
  {
    variant: 'default',
    size: 'default',
    disabled: false,
    type: 'button',
    class: '',
  }
);

const emit = defineEmits<{
  (e: 'click', event: MouseEvent): void;
}>();

const variantClasses = computed(() => {
  switch (props.variant) {
    case 'outline':
      return 'bg-white text-slate-700 hover:bg-slate-50 border border-slate-200/80 shadow-2xs';
    case 'ghost':
      return 'bg-transparent text-slate-600 hover:text-slate-900 hover:bg-slate-100/80';
    case 'destructive':
      return 'bg-rose-50 text-rose-600 hover:bg-rose-100/80 hover:text-rose-700';
    case 'secondary':
      return 'bg-slate-100 text-slate-700 hover:bg-slate-200/70';
    case 'default':
    default:
      return 'bg-indigo-600 text-white hover:bg-indigo-700 shadow-2xs active:scale-[0.98]';
  }
});

const sizeClasses = computed(() => {
  switch (props.size) {
    case 'sm':
      return 'h-8 px-3 text-xs rounded-lg gap-1.5';
    case 'lg':
      return 'h-11 px-5 text-base rounded-xl gap-2.5';
    case 'icon':
      return 'h-8 w-8 p-0 justify-center rounded-lg';
    case 'default':
    default:
      return 'h-9 px-4 text-sm font-medium rounded-lg gap-2';
  }
});
</script>

<template>
  <button
    :type="type"
    :disabled="disabled"
    class="inline-flex items-center justify-center font-medium transition-all duration-150 cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed select-none focus:outline-none focus-visible:ring-2 focus-visible:ring-indigo-500/30"
    :class="[variantClasses, sizeClasses, props.class]"
    @click="emit('click', $event)"
  >
    <slot />
  </button>
</template>
