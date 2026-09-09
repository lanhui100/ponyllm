<script setup lang="ts">
import { computed } from 'vue';

const props = withDefaults(
  defineProps<{
    name:
      | 'plus'
      | 'edit'
      | 'trash'
      | 'zap'
      | 'refresh'
      | 'chevron-down'
      | 'chevron-right'
      | 'copy'
      | 'check'
      | 'info'
      | 'lock'
      | 'brain'
      | 'sparkles'
      | 'server'
      | 'key'
      | 'activity'
      | 'cross'
      | 'file-text'
      | 'image'
      | 'video'
      | 'mic'
      | 'external';
    size?: number | string;
    class?: any;
  }>(),
  {
    size: 16,
    class: '',
  }
);

const pxSize = computed(() => (typeof props.size === 'number' ? `${props.size}px` : props.size));
</script>

<template>
  <svg
    xmlns="http://www.w3.org/2000/svg"
    :width="pxSize"
    :height="pxSize"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="2"
    stroke-linecap="round"
    stroke-linejoin="round"
    :class="['inline-block align-middle shrink-0', props.class]"
  >
    <!-- plus -->
    <path v-if="name === 'plus'" d="M5 12h14m-7-7v14" />
    <!-- edit -->
    <path
      v-else-if="name === 'edit'"
      d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z M15 5l4 4"
    />
    <!-- trash -->
    <path
      v-else-if="name === 'trash'"
      d="M3 6h18m-2 0v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6m3 0V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2m-6 5v6m4-6v6"
    />
    <!-- zap -->
    <path
      v-else-if="name === 'zap'"
      d="M4 14a1 1 0 0 1-.78-1.63l9.9-10.2a.5.5 0 0 1 .86.46l-1.92 6.02A1 1 0 0 0 13 10h7a1 1 0 0 1 .78 1.63l-9.9 10.2a.5.5 0 0 1-.86-.46l1.92-6.02A1 1 0 0 0 11 14z"
    />
    <!-- refresh -->
    <path
      v-else-if="name === 'refresh'"
      d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8m0 0V3m0 5h5m-5 4a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16m0 0v5m0-5h-5"
    />
    <!-- chevron-down -->
    <path v-else-if="name === 'chevron-down'" d="m6 9 6 6 6-6" />
    <!-- chevron-right -->
    <path v-else-if="name === 'chevron-right'" d="m9 18 6-6-6-6" />
    <!-- copy -->
    <g v-else-if="name === 'copy'">
      <rect width="14" height="14" x="8" y="8" rx="2" ry="2" />
      <path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2" />
    </g>
    <!-- check -->
    <path v-else-if="name === 'check'" d="M20 6 9 17l-5-5" />
    <!-- info -->
    <g v-else-if="name === 'info'">
      <circle cx="12" cy="12" r="10" />
      <path d="M12 16v-4m0-4h.01" />
    </g>
    <!-- lock -->
    <g v-else-if="name === 'lock'">
      <rect width="18" height="11" x="3" y="11" rx="2" ry="2" />
      <path d="M7 11V7a5 5 0 0 1 10 0v4" />
    </g>
    <!-- brain (thinking) -->
    <g v-else-if="name === 'brain'">
      <path d="M12 5a3 3 0 1 0-5.997.125 4 4 0 0 0-2.526 5.77 4 4 0 0 0 .556 6.588A4 4 0 1 0 12 18Z" />
      <path d="M12 5a3 3 0 1 1 5.997.125 4 4 0 0 1 2.526 5.77 4 4 0 0 1-.556 6.588A4 4 0 1 1 12 18Z" />
      <path d="M12 5v13" />
    </g>
    <!-- sparkles -->
    <g v-else-if="name === 'sparkles'">
      <path d="m12 3-1.912 5.813a2 2 0 0 1-1.275 1.275L3 12l5.813 1.912a2 2 0 0 1 1.275 1.275L12 21l1.912-5.813a2 2 0 0 1 1.275-1.275L21 12l-5.813-1.912a2 2 0 0 1-1.275-1.275L12 3Z" />
    </g>
    <!-- server -->
    <g v-else-if="name === 'server'">
      <rect width="20" height="8" x="2" y="2" rx="2" ry="2" />
      <rect width="20" height="8" x="2" y="14" rx="2" ry="2" />
      <line x1="6" x2="6.01" y1="6" y2="6" />
      <line x1="6" x2="6.01" y1="18" y2="18" />
    </g>
    <!-- key -->
    <g v-else-if="name === 'key'">
      <path d="m21 2-2 2m-1.5 1.5L14 9l-3-3-4 4 2 2-2 2-3-3-2 2a4.95 4.95 0 0 0 7 7l7.5-7.5" />
      <circle cx="7.5" cy="16.5" r="1.5" />
    </g>
    <!-- activity -->
    <path v-else-if="name === 'activity'" d="M22 12h-4l-3 9L9 3l-3 9H2" />
    <!-- cross -->
    <path v-else-if="name === 'cross'" d="M18 6 6 18M6 6l12 12" />
    <!-- file-text (text modality) -->
    <g v-else-if="name === 'file-text'">
      <path d="M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z" />
      <polyline points="14 2 14 8 20 8" />
      <line x1="16" x2="8" y1="13" y2="13" />
      <line x1="16" x2="8" y1="17" y2="17" />
      <line x1="10" x2="8" y1="9" y2="9" />
    </g>
    <!-- image (vision modality) -->
    <g v-else-if="name === 'image'">
      <rect width="18" height="18" x="3" y="3" rx="2" ry="2" />
      <circle cx="9" cy="9" r="2" />
      <path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21" />
    </g>
    <!-- video (video modality) -->
    <g v-else-if="name === 'video'">
      <path d="m22 8-6 4 6 4V8Z" />
      <rect width="14" height="12" x="2" y="6" rx="2" ry="2" />
    </g>
    <!-- mic (audio modality) -->
    <g v-else-if="name === 'mic'">
      <path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z" />
      <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
      <line x1="12" x2="12" y1="19" y2="22" />
    </g>
    <!-- external -->
    <path v-else-if="name === 'external'" d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6m4-3h6v6m-11 5L21 3" />
  </svg>
</template>
