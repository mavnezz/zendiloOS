import { fileURLToPath, URL } from 'node:url'

import tailwindcss from '@tailwindcss/vite'
import vue from '@vitejs/plugin-vue'
import { defineConfig } from 'vitest/config'

export default defineConfig({
  plugins: [vue(), tailwindcss()],
  resolve: {
    alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) },
  },
  server: {
    port: 5173,
    strictPort: true,
    // `just dev` serves the API here; the browser sees one origin, as with nginx.
    proxy: { '/api': { target: 'http://127.0.0.1:8780', ws: true } },
  },
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.ts'],
    // `npm run test:coverage` (just coverage-web); CI holds the thresholds.
    coverage: {
      provider: 'v8',
      include: ['src/**/*.{ts,vue}'],
      exclude: ['src/**/*.test.ts', 'src/**/*.d.ts', 'src/main.ts'],
      reporter: ['text-summary', 'html', 'json-summary'],
      reportsDirectory: '../dev/coverage/frontend',
      // Just below what the tests reach (11/2026): coverage may rise, not
      // fall unnoticed. The logic in .ts files is held higher than views.
      thresholds: {
        lines: 27,
        'src/**/*.ts': { lines: 38 },
      },
    },
  },
})
