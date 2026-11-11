import { defineConfig } from 'vite';
import { resolve } from 'node:path';

export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ['VITE_', 'TAURI_ENV_*'],
  build: {
    target: process.env.TAURI_ENV_PLATFORM === 'windows' ? 'chrome105' : 'safari13',
    rollupOptions: { input: { main: resolve(__dirname, 'index.html'), splash: resolve(__dirname, 'splash.html') } },
  },
});
