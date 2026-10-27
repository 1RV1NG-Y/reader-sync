import { readFileSync } from 'node:fs';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// The app's Content-Security-Policy lives in src-tauri/tauri.conf.json (Tauri sends it as a
// header). The same policy goes into index.html as a <meta> so the plain browser preview
// (`npm run dev` / `npm run preview`) runs under it too. Book content is rendered by foliate-js
// in sandboxed blob: iframes, which inherit this policy: book scripts never run.
const csp = JSON.parse(readFileSync(new URL('./src-tauri/tauri.conf.json', import.meta.url))).app.security.csp;

const cspMeta = () => ({
  name: 'reader-sync-csp',
  transformIndexHtml: (_html, ctx) => [{
    tag: 'meta',
    injectTo: 'head-prepend',
    attrs: {
      'http-equiv': 'Content-Security-Policy',
      // Dev only: React Fast Refresh injects an inline module script, and Vite's HMR uses a
      // websocket. Production builds get the policy unchanged.
      content: ctx.server
        ? csp.replace("script-src 'self'", "script-src 'self' 'unsafe-inline'")
          .replace("connect-src 'self'", "connect-src 'self' ws://127.0.0.1:5180")
        : csp,
    },
  }],
});

// foliate-js (vendor/foliate-js) is served and shipped unbundled at /foliate-js/: it loads its
// own modules and PDF.js assets with relative URLs, which bundling would break. Everything in
// vendor/ is copied as is into dist/.
export default defineConfig({
  plugins: [react(), cspMeta()],
  base: './',
  publicDir: 'vendor',
  server: { port: 5180, strictPort: true },
  build: { target: 'es2022' },
});
