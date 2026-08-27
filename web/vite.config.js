import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// `tavernsim serve` owns /api and /locales. In dev we proxy to it; the
// build is a relative-path bundle that the same binary serves out of
// web/dist, so the runtime needs no Node at all.
const backend = 'http://127.0.0.1:8765'

export default defineConfig({
  plugins: [react()],
  base: './',
  // The only consumer is a browser the user just launched from the app,
  // so there is no reason to down-compile to 2020 baselines — and doing
  // so makes esbuild warn on Spectrum's nested CSS.
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'es2022',
    cssTarget: 'chrome100',
    chunkSizeWarningLimit: 1200,
  },
  server: {
    port: 5173,
    proxy: {
      '/api': { target: backend, changeOrigin: false },
      '/locales': { target: backend, changeOrigin: false },
    },
  },
})
