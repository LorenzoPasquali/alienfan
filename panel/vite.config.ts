import { defineConfig } from 'vite';

// Tauri serves the build from disk; the dev server backs `tauri dev` and the
// browser preview (which uses a simulated daemon).
export default defineConfig({
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: { target: 'safari16', outDir: 'dist' },
});
