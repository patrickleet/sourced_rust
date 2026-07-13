import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [sveltekit()],
  server: {
    port: 5179,
    proxy: {
      '/graphql': process.env.WORKSHOP_BASE_URL || 'http://127.0.0.1:8791',
      '/product.': process.env.WORKSHOP_BASE_URL || 'http://127.0.0.1:8791',
      '/workshop_order.': process.env.WORKSHOP_BASE_URL || 'http://127.0.0.1:8791',
    },
  },
});
