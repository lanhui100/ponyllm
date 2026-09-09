<script setup lang="ts">
defineProps<{
  message: string | null;
}>();
</script>

<template>
  <Transition name="pony-toast">
    <div
      v-if="message"
      class="fixed bottom-6 left-1/2 -translate-x-1/2 z-50 max-w-md px-4 py-2.5 rounded-xl bg-slate-900/95 text-white text-sm shadow-lg text-center"
      data-testid="ui-toast"
      role="status"
    >
      {{ message }}
    </div>
  </Transition>
</template>

<style scoped>
.pony-toast-enter-active,
.pony-toast-leave-active {
  transition: opacity 0.2s ease, transform 0.2s ease;
}
.pony-toast-enter-from,
.pony-toast-leave-to {
  opacity: 0;
  transform: translate(-50%, 8px);
}
</style>
