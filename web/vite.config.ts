import { defineConfig } from 'vite';
import vue from '@vitejs/plugin-vue';

// Web console hosted at root '/' (and backwards compatible with '/app/').
export default defineConfig({
  base: '/',
  plugins: [vue()],
  server: {
    port: 5173,
    proxy: {
      // Dev only: vite :5173 -> serve :8080 so the browser stays same-origin.
      '/v1': 'http://127.0.0.1:8080',
      '/models': 'http://127.0.0.1:8080',
      '/health': 'http://127.0.0.1:8080',
      '/telemetry': 'http://127.0.0.1:8080',
      // Reserved for M3 (Admin API contract, /api/admin/*).
      '/api': 'http://127.0.0.1:8080',
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    chunkSizeWarningLimit: 600,
  },
});
