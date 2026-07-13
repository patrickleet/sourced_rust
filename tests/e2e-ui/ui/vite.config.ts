import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

const api = process.env.E2E_API_ORIGIN || process.env.E2E_BASE_URL || 'http://127.0.0.1:8791';

export default defineConfig({
  plugins: [sveltekit()],
  server: {
    port: 5180,
    proxy: {
      '/graphql': { target: api, changeOrigin: true, ws: true },
      '/todo.': api,
      '/chat.': api,
    },
  },
});
