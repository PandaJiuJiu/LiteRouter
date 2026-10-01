import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

// Kept separate from vite.config.js on purpose: the dev-server proxy is
// irrelevant under jsdom, and inheriting `server.proxy` here would only give
// the test runner a config it never reads.
export default defineConfig({
  plugins: [vue()],
  test: {
    environment: 'jsdom',
    include: ['tests/**/*.spec.js'],
    setupFiles: ['tests/setup.js'],
    // Element Plus registers ~60 globals; the DOM shims below cover the rest.
    restoreMocks: true,
  },
})