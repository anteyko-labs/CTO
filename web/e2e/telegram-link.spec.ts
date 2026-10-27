// Привязка Телеграма владельца ссылкой и QR (SPEC-36 §7, ADR-121): рядом с кодом — «Открыть бота»
// с кодом в ссылке и QR той же ссылки. Бот в проверках выключен — ответы сервера подменяются.
import { expect, test } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'

test('код привязки: кнопка «Открыть бота» ведёт на t.me с кодом, рядом QR', async ({ page }) => {
  await page.route('**/api/v1/settings/telegram', (r) =>
    r.request().method() === 'GET' ? r.fulfill({ json: { enabled: true, linked: false, linked_at: null } }) : r.continue(),
  )
  await page.route('**/api/v1/settings/telegram/code', (r) =>
    r.fulfill({ json: { code: '042917', expires_at: new Date(Date.now() + 15 * 60_000).toISOString() } }),
  )
  await page.route('**/api/v1/telegram/bot', (r) => r.fulfill({ json: { username: 'avtodom_test_bot' } }))
  await page.route('**/api/v1/settings/telegram/questions', (r) =>
    r.fulfill({
      json: { enabled: false, daily_limit: null, today: { questions: 0, input_tokens: 0, output_tokens: 0 }, month: { questions: 0, input_tokens: 0, output_tokens: 0 } },
    }),
  )

  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()

  await page.goto('/settings/telegram')
  await page.getByRole('button', { name: 'Получить код' }).click()
  await expect(page.getByText('042917', { exact: true })).toBeVisible()
  const open = page.getByRole('link', { name: 'Открыть бота' })
  await expect(open).toHaveAttribute('href', 'https://t.me/avtodom_test_bot?start=042917')
  await expect(open).toHaveAttribute('target', '_blank')
  const qr = page.getByTestId('bot-qr')
  await expect(qr).toBeVisible()
  await expect(qr).toHaveAttribute('src', /^data:image\/png;base64,/)
})
