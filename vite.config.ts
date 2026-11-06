import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';
import { pdfjsResources } from './scripts/vite-pdfjs-resources.mjs';

export default defineConfig({
  plugins: [react(), pdfjsResources()],
  test: {
    globals: false,
    environment: 'node',
    include: ['src/**/*.test.{ts,tsx}'],
    passWithNoTests: false,
  },
  server: {
    open: false,
    port: 5173,
  },
});
