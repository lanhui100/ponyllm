<template>
  <svg
    :width="size"
    :height="size"
    viewBox="0 0 64 64"
    fill="none"
    xmlns="http://www.w3.org/2000/svg"
    class="shrink-0 select-none"
  >
    <defs>
      <linearGradient :id="`monoGrad-${id}`" x1="12" y1="10" x2="54" y2="54" gradientUnits="userSpaceOnUse">
        <stop stop-color="#FA541C" />
        <stop offset="0.5" stop-color="#F97316" />
        <stop offset="1" stop-color="#FB923C" />
      </linearGradient>
      <filter :id="`softShadow-${id}`" x="-10%" y="-10%" width="120%" height="120%">
        <feDropShadow dx="0" dy="4" stdDeviation="4" flood-color="#0F172A" flood-opacity="0.12" />
      </filter>
    </defs>

    <!-- 现代深灰外衬底座 -->
    <rect width="64" height="64" rx="16" fill="#0F172A" :filter="`url(#softShadow-${id})`" />
    <rect x="0.75" y="0.75" width="62.5" height="62.5" rx="15.25" stroke="#334155" stroke-width="1.5" stroke-opacity="0.5" />

    <!-- 字母 P 与天马同构造型 -->
    <g :fill="`url(#monoGrad-${id})`">
      <!-- 竖轴主干 -->
      <path d="M18 16C18 14.8954 18.8954 14 20 14H24C25.1046 14 26 14.8954 26 16V48C26 49.1046 25.1046 50 24 50H20C18.8954 50 18 49.1046 18 48V16Z" />
      <!-- 马首与 P 环形曲面 -->
      <path
        fill-rule="evenodd"
        clip-rule="evenodd"
        d="M26 14H36C44.8366 14 50 19.5 50 27C50 33.5 45.5 38.5 38 39.5L46 49C46.6 49.8 45.8 51 44.8 51H38.5C37.8 51 37.1 50.6 36.7 50L29.5 40H26V14ZM36 32C40.4183 32 43 29.5 43 25.5C43 21.5 40.4183 19 36 19H26V32H36Z"
      />
      <!-- 前额马耳 -->
      <path d="M34 9C34 9 37 11 36 14H31L34 9Z" />
    </g>

    <!-- 马眼高光 -->
    <circle cx="37.5" cy="24.5" r="2.5" fill="#FFFFFF" />
  </svg>
</template>

<script setup lang="ts">
import { useId } from 'vue';

withDefaults(
  defineProps<{
    size?: number | string;
  }>(),
  {
    size: 34,
  },
);

const id = useId();
</script>
