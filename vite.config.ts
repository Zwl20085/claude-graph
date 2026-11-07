/// <reference types="vitest" />
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

const DEV_PORT = 1420;
const COVERAGE_THRESHOLD = 80;

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  envPrefix: ['VITE_', 'TAURI_ENV_*'],
  server: {
    port: DEV_PORT,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] },
  },
  build: {
    outDir: 'dist',
    target: 'es2021',
  },
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.ts', 'src/**/*.test.tsx'],
    coverage: {
      provider: 'v8',
      include: ['src/lib/**/*.ts'],
      exclude: ['src/lib/**/*.test.ts', 'src/lib/ipc.ts'],
      thresholds: { lines: COVERAGE_THRESHOLD },
    },
  },
});
