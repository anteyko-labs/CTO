// «Настройки → Касса» и скидка на кассе (задача 003 Н2–Н4): раскладка кнопок режима кассы
// сохраняется и после перезагрузки, «Ещё действия…» открывает окно; скидка 10 % на весь чек;
// администратор без права — кнопка неактивна, с правом — проводит в пределе, свыше — сервер не
// проводит; экран чека в режиме кассы — без себестоимости. В конце — настройки как были.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e3d1' }
const RUN = Date.now().toString().slice(-8)
const PICKER = /Сканируйте штрихкод/
const money = (s: string) => new RegExp(s.replace(/ /g, '\\s'))

test.use({ viewport: { width: 1366, height: 768 }, hasTouch: true })

async function login(page: Page, user = OWNER, password = OWNER_PASSWORD) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(user)
  await page.getByLabel('Пароль').fill(password)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'POST' | 'PUT', path: string, data: unknown): Promise<T> {
  const res = await request.fetch(`/api/v1${path}`, { method, data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

/** Настройка кассы как по умолчанию. */
const resetCashier = (request: APIRequestContext) => api(request, 'PUT', '/settings/cashier', { op_id: crypto.randomUUID() })

/** Товар по 1 000 с на складе и открытая смена с кассиром. */
async function goodsAndShift(request: APIRequestContext, tag: string): Promise<{ name: string; id: string }> {
  const cat = await api<{ id: string }>(request, 'POST', '/categories', { name: `Скидка ${tag} ${RUN}`, kind: 'other' })
  const name = `Коврик ${tag} ${RUN}`
  const p = await api<{ id: string }>(request, 'POST', '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name, sale_price_tyiyn: 100_000 })
  await api(request, 'POST', '/receipts', { op_id: crypto.randomUUID(), paid_from: await ownerSafe(request), lines: [{ product_id: p.id, qty: 10, cost_tyiyn: 300_000 }] })
  const cur = (await (await request.get('/api/v1/shifts/current')).json()) as { id: string } | null
  if (!cur) {
    const emp = await api<{ id: string }>(request, 'POST', '/employees', { full_name: `Кассир скидки ${RUN}`, is_cashier: true, pin: '1593' })
    await api(request, 'POST', '/shifts/open', { op_id: crypto.randomUUID(), cashier_employee_id: emp.id, cashier_pin: '1593' })
  }
  return { name, id: p.id }
}

async function addToCart(page: Page, name: string) {
  await page.getByPlaceholder(PICKER).fill(name)
  await page.getByRole('button', { name: new RegExp(`^${name}`) }).first().click()
  await expect(page.getByTestId('cart')).toContainText(name)
}

test('настройки кассы: место кнопок, «Ещё действия…», набор «Частое — у итога» и «Как было»', async ({ page }) => {
  await login(page)
  await resetCashier(page.request)
  await page.goto('/settings')
  await page.getByRole('link', { name: /Кнопки режима кассы и их место/ }).click()
  await expect(page.getByRole('heading', { name: 'Касса', exact: true })).toBeVisible()
  const layout = page.getByTestId('cashier-layout')
  await layout.getByLabel('Место: Аккумуляторы на вес').selectOption('hidden')
  await layout.getByLabel('Место: Баллы клиента').selectOption('total')
  await layout.getByLabel('Место: Мелкий расход из кассы').selectOption('more')
  await expect(page.getByTestId('layout-preview')).toContainText('Скрыто: Аккумуляторы на вес')
  await page.getByRole('button', { name: 'Сохранить' }).click()
  await expect(page.getByText('Настройки кассы сохранены')).toBeVisible()
  await page.reload()
  await expect(page.getByTestId('cashier-layout').getByLabel('Место: Аккумуляторы на вес')).toHaveValue('hidden')

  // Режим кассы: кнопки на своих местах; «Ещё действия…» — окно с перенесённой кнопкой.
  await page.goto('/')
  await page.getByRole('button', { name: 'Режим кассы', exact: true }).tap()
  await expect(page.getByTestId('cashier-mode-bar')).toBeVisible()
  for (const pass of [1, 2]) {
    const actions = page.getByTestId('mode-actions')
    await expect(actions).toContainText('Принять оплату долга')
    await expect(actions).not.toContainText('Аккумуляторы на вес')
    await expect(actions).not.toContainText('Мелкий расход')
    await expect(page.getByTestId('mode-total-buttons')).toContainText('Баллы клиента')
    await expect(page.getByTestId('mode-pay-buttons')).not.toContainText('Баллы клиента')
    await actions.getByRole('button', { name: 'Ещё действия…' }).tap()
    const more = page.getByTestId('more-actions-modal')
    await expect(more.getByRole('button', { name: /Мелкий расход из кассы/ })).toBeVisible()
    await page.getByRole('dialog', { name: 'Ещё действия' }).getByRole('button', { name: 'Закрыть' }).tap()
    await expect(more).toHaveCount(0)
    if (pass === 1) await page.reload()
  }
  await page.getByTestId('cashier-mode-bar').getByRole('button', { name: 'Выйти из режима кассы' }).tap()

  // Готовый набор и «Как было».
  await page.goto('/settings/cashier')
  await page.getByRole('button', { name: 'Частое — у итога' }).click()
  const preview = page.getByTestId('layout-preview')
  await expect(preview).toContainText('Скидка · Баллы клиента · Сдал аккумулятор')
  await expect(preview).toContainText('Окно «Ещё действия…»Принять оплату долга')
  await page.getByRole('button', { name: 'Как было' }).click()
  await expect(preview).toContainText('Под поискомПринять оплату долга · Мелкий расход из кассы')
  await page.getByRole('button', { name: 'Сохранить' }).click()
  await expect(page.getByText('Настройки кассы сохранены')).toBeVisible()
  await resetCashier(page.request)
})

test('скидка на кассе: владелец 10 % на чек, экран чека у кассы без себестоимости; администратор — по праву и пределу', async ({ page, browser }) => {
  await login(page)
  await resetCashier(page.request)
  const { name, id: pid } = await goodsAndShift(page.request, 'В')

  // Владелец в режиме кассы: «Скидка» → 10 % на весь чек.
  await page.goto('/')
  await page.getByRole('button', { name: 'Режим кассы', exact: true }).tap()
  await addToCart(page, name)
  await page.getByTestId('discount-open').tap()
  const dialog = page.getByRole('dialog', { name: 'Скидка' })
  await dialog.getByRole('button', { name: '10 %', exact: true }).tap()
  await expect(dialog.getByTestId('discount-preview')).toContainText(money('Скидка −100 с · итого 900 с'))
  await dialog.getByRole('button', { name: 'Применить' }).tap()
  await expect(dialog).toHaveCount(0)
  await expect(page.getByTestId('check-discount')).toContainText(money('Скидка −100 с'))
  await expect(page.getByTestId('pay-top')).toContainText(money('900 с'))
  // До проведения скидку можно убрать и дать снова.
  await page.getByTestId('discount-open').tap()
  await dialog.getByRole('button', { name: 'Убрать скидку' }).tap()
  await expect(page.getByTestId('check-discount')).toHaveCount(0)
  await page.getByTestId('discount-open').tap()
  await dialog.getByRole('button', { name: '10 %', exact: true }).tap()
  await dialog.getByRole('button', { name: 'Применить' }).tap()
  await page.getByRole('button', { name: 'Без сдачи' }).tap()
  await page.getByRole('button', { name: /Провести чек/ }).tap()
  await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()

  // Экран чека с кассы — без себестоимости и прибыли (задача 003 Н4); вне режима — с ними.
  await page.getByRole('link', { name: 'Открыть', exact: true }).tap()
  await expect(page.getByText(money('Итого: 900 с'))).toBeVisible()
  await expect(page.getByRole('columnheader', { name: 'Себестоимость' })).toHaveCount(0)
  await expect(page.getByText('Валовая прибыль')).toHaveCount(0)
  await page.evaluate(() => localStorage.removeItem('avtodom.cashier_mode'))
  await page.reload()
  await expect(page.getByRole('columnheader', { name: 'Себестоимость' })).toBeVisible()

  // Администратор без права «Скидка в чеке»: кнопка неактивна, подпись — «скидку даёт владелец».
  const admin = `skidka${RUN}`
  const password = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  const user = await api<{ id: string }>(page.request, 'POST', '/users', { login: admin, password, full_name: `Скидка ${RUN}`, role: 'admin' })
  const ctx = await browser.newContext({ viewport: { width: 1366, height: 768 } })
  const a = await ctx.newPage()
  await login(a, admin, password)
  await a.goto('/')
  await addToCart(a, name)
  await expect(a.getByTestId('discount-open')).toBeDisabled()
  await expect(a.getByTestId('discount-open')).toHaveText('Скидка — даёт владелец')

  // Владелец даёт право и ставит предел 5 %: 10 % окно не применяет, 5 % — проводится.
  const users = (await (await page.request.get('/api/v1/users')).json()) as { id: string; permissions?: string[] }[]
  const perms = users.find((u) => u.id === user.id)?.permissions ?? []
  await api(page.request, 'PUT', `/users/${user.id}/permissions`, { op_id: crypto.randomUUID(), permissions: [...perms, 'discount'] })
  await api(page.request, 'PUT', '/settings/cashier', { op_id: crypto.randomUUID(), admin_limit_pct: 5 })
  await a.reload()
  await expect(a.getByTestId('cart')).toContainText(name)
  await a.getByTestId('discount-open').click()
  const ad = a.getByRole('dialog', { name: 'Скидка' })
  await expect(ad).toContainText('Предел скидки администратора: 5 %')
  await ad.getByRole('button', { name: '10 %', exact: true }).click()
  await expect(ad).toContainText('Больше предела администратора (5 %)')
  await expect(ad.getByRole('button', { name: 'Применить' })).toBeDisabled()
  await ad.getByRole('button', { name: '5 %', exact: true }).click()
  await ad.getByRole('button', { name: 'Применить' }).click()
  await expect(a.getByTestId('check-discount')).toContainText(money('Скидка −50 с'))
  await a.getByRole('button', { name: 'Провести чек' }).first().click()
  await expect(a.getByText(/Чек № \d+ проведён/)).toBeVisible()

  // Свыше предела сервер не проводит (I12): 403 с причиной.
  const shift = (await (await a.request.get('/api/v1/shifts/current')).json()) as { cashier_employee_id: string }
  const res = await a.request.post('/api/v1/sales', {
    data: {
      op_id: crypto.randomUUID(),
      sale_type: 'takeaway',
      cashier_id: shift.cashier_employee_id,
      lines: [{ kind: 'piece', product_id: pid, qty: 1, unit_price_tyiyn: 85_000 }],
      payments: [{ method: 'cash', amount_tyiyn: 85_000 }],
    },
    headers: DEVICE,
  })
  expect(res.status(), await res.text()).toBe(403)
  expect(await res.text()).toContain('больше предела администратора (5 %)')
  await ctx.close()
  await resetCashier(page.request)
})
