<template>
  <div class="relative min-h-screen">
    <!-- 全局多色彩撞色渐变底图层 (高饱和紧凑撞色：暖日橙/珊瑚橙 + 板岩青/湖水绿) -->
    <div class="global-gradient-container" aria-hidden="true">
      <div class="ambient-orb ambient-orb-orange-main" />
      <div class="ambient-orb ambient-orb-teal-main" />
      <div class="ambient-orb ambient-orb-orange-soft" />
      <div class="ambient-orb ambient-orb-teal-deep" />
      <div class="ambient-noise-layer" />
    </div>

    <!-- 顶层主内容区（保持相对定位以浮于背景光斑之上） -->
    <div class="relative z-1 min-h-screen">
      <router-view />
    </div>
  </div>
</template>
