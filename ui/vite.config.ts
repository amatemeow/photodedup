import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// Dev: `npm run dev` serves the UI on :5173 and proxies the API to a
// running `photodedup serve` (set PHOTODEDUP_SERVER=http://127.0.0.1:<port>;
// open http://localhost:5173/?token=<token> -- the token is in that
// server's startup URL). Production: `npm run build`, and the Rust server
// serves ui/dist directly -- same origin, so no proxy or CORS involved.
const target = process.env.PHOTODEDUP_SERVER ?? 'http://127.0.0.1:0';

export default defineConfig({
  plugins: [svelte()],
  base: './',
  server: {
    proxy: {
      '/api': target,
      '/files': target,
    },
  },
});
