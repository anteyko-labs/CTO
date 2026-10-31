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

// Задача 003, Н6: накладные в бот от администратора — привязка заметна в «Пользователях» и в
// «Телеграм и баллы», там же склейка фото.
test('Телеграм администратора: «Привязать» в колонке и карточка «Накладные в бот присылают»', async ({ page }) => {
  const RUN = Date.now().toString().slice(-8)
  await page.route('**/api/v1/settings/telegram', (r) =>
    r.request().method() === 'GET' ? r.fulfill({ json: { enabled: true, linked: false, linked_at: null } }) : r.continue(),
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
  const res = await page.request.post('/api/v1/users', {
    data: { login: `nakl${RUN}`, password: `pass-${RUN}-x1`, full_name: `Накладчик ${RUN}`, role: 'admin' },
    headers: { 'X-Device-Id': '00000000-0000-4000-8000-00000000e36b' },
  })
  expect(res.ok(), await res.text()).toBeTruthy()

  // «Пользователи»: в колонке «Телеграм» — кнопка, она открывает код и QR.
  await page.goto('/users')
  const row = page.getByRole('row').filter({ hasText: `nakl${RUN}` })
  await row.getByTestId('user-telegram-btn').click()
  await expect(page.getByTestId('user-telegram-modal')).toContainText('📷 Накладная')
  await page.getByTestId('user-telegram-code-btn').click()
  await expect(page.getByTestId('user-telegram-code')).toHaveText(/^\d{6}$/)
  await expect(page.getByRole('link', { name: 'Открыть бота' })).toHaveAttribute('href', /start=\d{6}$/)
  await page.getByTestId('user-telegram-modal').getByRole('button', { name: 'Закрыть' }).click()

  // «Телеграм и баллы»: администраторы, их право «Приход товара», склейка фото.
  await page.goto('/settings/telegram')
  const card = page.getByTestId('bot-receipts-card')
  await expect(card.getByTestId('bot-receipts-admin').filter({ hasText: `nakl${RUN}` })).toContainText('Телеграм не привязан')
  await expect(page.locator('#bot-merge-minutes')).toHaveValue('0')
  await page.locator('#bot-merge-minutes').fill('10')
  await card.getByRole('button', { name: 'Сохранить' }).click()
  await expect(page.getByText('Сохранено')).toBeVisible()
  await page.reload()
  await expect(page.locator('#bot-merge-minutes')).toHaveValue('10')
  // Вернуть как было — каждое фото отдельно.
  await page.locator('#bot-merge-minutes').fill('0')
  await page.getByTestId('bot-receipts-card').getByRole('button', { name: 'Сохранить' }).click()
  await expect(page.getByText('Сохранено')).toBeVisible()
})
