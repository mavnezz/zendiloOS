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
      // The tests' own helpers (stand-in server, mounting) are not measured.
      exclude: [
        'src/**/*.test.ts',
        'src/**/*.d.ts',
        'src/main.ts',
        'src/shared/testing/**',
        'src/shared/test-support/**',
      ],
      reporter: ['text-summary', 'html', 'json-summary'],
      reportsDirectory: '../dev/coverage/frontend',
      // Just below what the tests reach (11/2026: lines 94 %, statements
      // 93 %, functions 89 %, branches 85 %, .ts lines 97 %): coverage may
      // rise, not fall unnoticed. The logic in .ts files is held higher.
      thresholds: {
        lines: 93,
        statements: 92,
        functions: 88,
        branches: 84,
        'src/**/*.ts': { lines: 96 },
      },
    },
  },
})
