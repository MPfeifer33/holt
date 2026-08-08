import { defineConfig } from 'vitest/config';
import { sveltekit } from '@sveltejs/kit/vite';

export default defineConfig({
  plugins: [sveltekit()],
  test: {
    include: ['src/**/*.test.ts'],
    environment: 'node',
    globals: true,
  },
  resolve: {
    alias: {
      '$lib': '/src/lib',
      '@tauri-apps/api/event': '/src/lib/test-utils/tauri-event-mock.ts',
    },
  },
});
