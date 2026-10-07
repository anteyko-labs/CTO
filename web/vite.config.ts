import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'
import { VitePWA } from 'vite-plugin-pwa'

export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
    // Оболочка приложения кэшируется, чтобы касса открывалась без сети (SPEC-09, ADR-038).
    VitePWA({
      registerType: 'prompt',
      manifest: {
        name: 'Avtodom',
        short_name: 'Avtodom',
        start_url: '/',
        display: 'standalone',
        background_color: '#0f172a',
        theme_color: '#0f172a',
      },
      workbox: {
        globPatterns: ['**/*.{js,css,html,svg,woff2}'],
        // Запросы к API в кэш не идут: данные берутся из снимка в IndexedDB.
        navigateFallbackDenylist: [/^\/api\//],
      },
    }),
  ],
  server: {
    proxy: { '/api': 'http://127.0.0.1:8080' },
  },
})
