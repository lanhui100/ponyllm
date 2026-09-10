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
      return 'bg-white text-slate-700 hover:bg-slate-50 border border-slate-200 shadow-2xs';
    case 'ghost':
      return 'bg-transparent text-slate-600 hover:text-slate-900 hover:bg-slate-100/80';
    case 'destructive':
      return 'bg-rose-600 text-white hover:bg-rose-700 shadow-2xs border-0';
    case 'secondary':
      return 'bg-slate-100 text-slate-700 hover:bg-slate-200/80 border border-slate-200/50';
    case 'default':
    default:
      return 'bg-slate-900 text-white hover:bg-slate-800 shadow-2xs';
  }
});

const sizeClasses = computed(() => {
  switch (props.size) {
    case 'sm':
      return 'h-8.5 px-3 text-[13px] font-medium rounded-lg gap-1.5';
    case 'lg':
      return 'h-11 px-5 text-base font-medium rounded-xl gap-2.5';
    case 'icon':
      return 'h-8.5 w-8.5 p-0 justify-center rounded-lg';
    case 'default':
    default:
      return 'h-9.5 px-4 text-sm font-medium rounded-lg gap-2';
  }
});
</script>

<template>
  <button
    :type="type"
    :disabled="disabled"
    class="inline-flex items-center justify-center font-medium transition-all duration-150 cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed select-none enabled:active:scale-[0.98] focus:outline-none focus-visible:ring-2 focus-visible:ring-orange-500/50 focus-visible:ring-offset-1"
    :class="[variantClasses, sizeClasses, props.class]"
    @click="emit('click', $event)"
  >
    <slot />
  </button>
</template>
