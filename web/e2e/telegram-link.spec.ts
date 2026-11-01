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

// SPEC-40: администратор в боте — в окне «Телеграм» у администратора «Что видит в боте» по его
// правам и события, которые ему приходят (по умолчанию — ничего). Бот подменён.
test('Телеграм администратора: «Что видит в боте» и события в бот', async ({ page }) => {
  const RUN = Date.now().toString().slice(-8)
  await page.route('**/api/v1/telegram/bot', (r) => r.fulfill({ json: { username: 'avtodom_test_bot' } }))
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
  const res = await page.request.post('/api/v1/users', {
    data: { login: `botv${RUN}`, password: `pass-${RUN}-x1`, full_name: `Ботовед ${RUN}`, role: 'admin' },
    headers: { 'X-Device-Id': '00000000-0000-4000-8000-00000000e36c' },
  })
  expect(res.ok(), await res.text()).toBeTruthy()

  await page.goto('/users')
  const row = page.getByRole('row').filter({ hasText: `botv${RUN}` })
  await row.getByTestId('user-telegram-btn').click()
  const modal = page.getByTestId('user-telegram-modal')
  await expect(modal.getByTestId('user-telegram-state')).toContainText('не привязан')
  const panel = modal.getByTestId('admin-bot-panel')
  // Права по умолчанию: смена и остаток есть, «Занести долг» — нет права «Старые долги из тетради».
  await expect(panel.getByTestId('admin-bot-item-shift')).toHaveAttribute('data-on', 'yes')
  await expect(panel.getByTestId('admin-bot-item-stock')).toHaveAttribute('data-on', 'yes')
  await expect(panel.getByTestId('admin-bot-item-add_debt')).toHaveAttribute('data-on', 'no')
  await expect(panel.getByTestId('admin-bot-item-add_debt')).toContainText('Нет права: Старые долги из тетради')
  await expect(panel.getByTestId('admin-bot-items')).not.toContainText('прибыл')
  // Лента: по умолчанию ничего; включили «Смена открыта» — сохраняется.
  await expect(panel.getByTestId('admin-bot-count')).toContainText('включено 0 из')
  await panel.getByTestId('admin-bot-event-shift.open').check()
  await expect(page.getByText('«Смена открыта» — приходит администратору')).toBeVisible()
  await expect(panel.getByTestId('admin-bot-count')).toContainText('включено 1 из')
  await modal.getByRole('button', { name: 'Закрыть' }).click()
  await row.getByTestId('user-telegram-btn').click()
  await expect(page.getByTestId('admin-bot-event-shift.open')).toBeChecked()
  await expect(page.getByTestId('admin-bot-event-receipt.post')).not.toBeChecked()

  // SPEC-40 «Дополнено: помощник»: по умолчанию включён (задача 005, ответ 9б); владелец
  // выключает каждому и ставит лимит.
  const helper = page.getByTestId('admin-bot-helper')
  await expect(helper.getByTestId('admin-bot-helper-on')).toBeChecked()
  await expect(helper.getByTestId('admin-bot-helper-limit')).toHaveValue('50')
  await expect(helper).not.toContainText('прибыль владельца')
  await helper.getByTestId('admin-bot-helper-on').uncheck()
  await expect(page.getByText('Помощник в боте выключен')).toBeVisible()
  await expect(helper.getByTestId('admin-bot-helper-status')).toContainText('Выключен')
  await helper.getByTestId('admin-bot-helper-on').check()
  await expect(page.getByText('Помощник в боте включён')).toBeVisible()
  await expect(helper.getByTestId('admin-bot-helper-status')).toContainText(/сегодня 0 из 50|не настроен/)
  await helper.getByTestId('admin-bot-helper-limit').fill('501')
  await expect(helper.getByText('от 1 до 500')).toBeVisible()
  await helper.getByTestId('admin-bot-helper-limit').fill('20')
  await helper.getByRole('button', { name: 'Сохранить' }).click()
  await expect(page.getByText('Лимит вопросов сохранён')).toBeVisible()
  await page.getByTestId('user-telegram-modal').getByRole('button', { name: 'Закрыть' }).click()
  await row.getByTestId('user-telegram-btn').click()
  await expect(page.getByTestId('admin-bot-helper-on')).toBeChecked()
  await expect(page.getByTestId('admin-bot-helper-limit')).toHaveValue('20')
  await expect(page.getByTestId('admin-bot-helper-status')).toContainText(/из 20|не настроен/)
})

// ADR-{h18}, SPEC-40 §10–§11: сотрудник со входом сам привязывает свой Телеграм в «Мой Телеграм»;
// владелец выключает это в «Настройки → Телеграм». Бот в проверках выключен — «включён» подставляется
// в ответ сервера, код и всё остальное — настоящие.
test('«Мой Телеграм»: сотрудник берёт код сам, владелец выключает самостоятельную привязку', async ({ page }) => {
  const RUN = Date.now().toString().slice(-8)
  const LOGIN = `moitg${RUN}`
  const PASSWORD = `pass-${RUN}-x1`
  await page.route('**/api/v1/telegram/bot', (r) => r.fulfill({ json: { username: 'avtodom_test_bot' } }))
  await page.route('**/api/v1/me/telegram', async (r) => {
    if (r.request().method() !== 'GET') return r.continue()
    const res = await r.fetch()
    return r.fulfill({ response: res, json: { ...(await res.json()), enabled: true } })
  })
  const login = async (who: string, password: string) => {
    await page.goto('/login')
    await page.getByLabel('Логин').fill(who)
    await page.getByLabel('Пароль').fill(password)
    await page.getByRole('button', { name: 'Войти' }).click()
  }
  await login(OWNER, OWNER_PASSWORD)
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
  const res = await page.request.post('/api/v1/users', {
    data: { login: LOGIN, password: PASSWORD, full_name: `Свой Телеграм ${RUN}`, role: 'admin' },
    headers: { 'X-Device-Id': '00000000-0000-4000-8000-00000000e36d' },
  })
  expect(res.ok(), await res.text()).toBeTruthy()
  // Повторный прогон после сбоя: выключатель — в исходное «да».
  const reset = await page.request.put('/api/v1/settings/telegram/admins', {
    data: { op_id: crypto.randomUUID(), self_link: true },
    headers: { 'X-Device-Id': '00000000-0000-4000-8000-00000000e36d' },
  })
  expect(reset.ok(), await reset.text()).toBeTruthy()

  // Сотрудник входит: подсказка «Подключите бот», по ней — «Мой Телеграм».
  await page.context().clearCookies()
  await login(LOGIN, PASSWORD)
  await expect(page.getByTestId('bot-hint')).toContainText('Подключите бот')
  await page.getByTestId('bot-hint').getByRole('link', { name: 'Мой Телеграм' }).click()
  await expect(page).toHaveURL(/\/my-telegram$/)
  await expect(page.getByTestId('my-telegram-state')).toHaveText('Телеграм не привязан')
  await expect(page.getByTestId('my-telegram-item-shift')).toHaveAttribute('data-on', 'yes')
  await expect(page.getByTestId('my-telegram-items')).not.toContainText('прибыл')
  await page.getByTestId('my-telegram-code-btn').click()
  await expect(page.getByTestId('my-telegram-code')).toHaveText(/^\d{6}$/)
  const code = (await page.getByTestId('my-telegram-code').textContent()) ?? ''
  await expect(page.getByRole('link', { name: 'Открыть бота' })).toHaveAttribute('href', `https://t.me/avtodom_test_bot?start=${code}`)

  // Владелец выключает — у сотрудника «Привязку делает владелец».
  await page.context().clearCookies()
  await login(OWNER, OWNER_PASSWORD)
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
  await page.goto('/settings/telegram')
  const self = page.getByTestId('telegram-admins-self').getByRole('checkbox')
  await expect(self).toBeChecked()
  // Флажок меняется по ответу сервера — щелчок, затем ожидание.
  await self.click()
  await expect(page.getByText('Привязку сотрудникам делаете вы')).toBeVisible()
  await expect(self).not.toBeChecked()
  await page.context().clearCookies()
  await login(LOGIN, PASSWORD)
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
  await page.goto('/my-telegram')
  await expect(page.getByTestId('my-telegram-owner')).toContainText('Привязку делает владелец')
  await expect(page.getByTestId('my-telegram-code-btn')).toHaveCount(0)

  // Вернуть как было — сотрудники привязывают сами.
  await page.context().clearCookies()
  await login(OWNER, OWNER_PASSWORD)
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
  await page.goto('/settings/telegram')
  await page.getByTestId('telegram-admins-self').getByRole('checkbox').click()
  await expect(page.getByTestId('telegram-admins-self').getByRole('checkbox')).toBeChecked()
})

