import { defineConfig } from '@playwright/test'

// Сквозные проверки в браузере. Нужен запущенный сервер с собранным клиентом
// и владелец с логином/паролем из E2E_LOGIN / E2E_PASSWORD.
export default defineConfig({
  testDir: './e2e',
  timeout: 60_000,
  retries: process.env.CI ? 1 : 0,
  workers: 1,
  reporter: process.env.CI ? [['list'], ['html', { open: 'never' }]] : 'list',
  use: {
    baseURL: process.env.E2E_BASE_URL ?? 'http://127.0.0.1:8080',
    channel: process.env.E2E_CHANNEL || undefined,
    locale: 'ru-RU',
    timezoneId: 'Asia/Bishkek',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
})
