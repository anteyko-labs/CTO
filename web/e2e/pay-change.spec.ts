// «Изменить оплату» чека (SPEC-37 §2, ADR-132): окно распределяет итог по способам, сервер
// показывает «Что изменится» пробным проведением, «Сохранить» открывает новый чек со ссылкой на
// исходный. Товары и цены не меняются.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ownerSafe } from './safe'
import { ensureShift } from './cashier'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e37a' }
const RUN = Date.now().toString().slice(-8)
const money = (text: string) => new RegExp(text.replace(/ /g, '\\s'))

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function apiPost<T>(request: APIRequestContext, path: string, data: unknown): Promise<T> {
  const res = await request.post(`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

test('изменить оплату: распределение, «Что изменится», новый чек', async ({ page }) => {
  await login(page)
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Кат оплаты ${RUN}`, kind: 'other' })
  const item = await apiPost<{ id: string }>(page.request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Фильтр оплаты ${RUN}`,
    sale_price_tyiyn: 50_000,
  })
  await apiPost(page.request, '/receipts', {
    op_id: crypto.randomUUID(),
    paid_from: await ownerSafe(page.request),
    lines: [{ product_id: item.id, qty: 5, cost_tyiyn: 100_000 }],
  })
  const cashier = await apiPost<{ id: string }>(page.request, '/employees', { full_name: `Кассир оплаты ${RUN}`, is_cashier: true, pin: '1234' })
  // Онлайн-чек — только при открытой смене (задача 005 Н4).
  await ensureShift(page.request, cashier.id)
  const sale = await apiPost<{ id: string; number: number }>(page.request, '/sales', {
    op_id: crypto.randomUUID(),
    sale_type: 'takeaway',
    cashier_id: cashier.id,
    lines: [{ kind: 'piece', product_id: item.id, qty: 1, unit_price_tyiyn: 50_000 }],
    payments: [{ method: 'card', amount_tyiyn: 50_000 }],
  })

  await page.goto(`/sales/${sale.id}`)
  await page.getByTestId('pay-change-button').click()
  const box = page.getByTestId('pay-change')
  await expect(box).toContainText('Оплата сейчас')
  await expect(page.getByTestId('pay-card')).toHaveValue('500')

  // Перекладываем: 300 наличными, 200 картой; пока не сошлось — «Осталось распределить».
  await page.getByTestId('pay-card').fill('')
  await page.getByTestId('pay-cash').fill('300')
  await expect(page.getByTestId('pay-rest')).toContainText(money('200 с'))
  await expect(page.getByTestId('pay-save')).toBeDisabled()
  await page.getByTestId('pay-card').fill('200')
  await expect(page.getByTestId('pay-rest')).toHaveCount(0)
  // «Что изменится» — от сервера, пробным проведением.
  await expect(page.getByTestId('pay-effect')).toContainText('Что изменится')
  // Причина обязательна.
  await expect(page.getByTestId('pay-save')).toBeDisabled()
  await page.getByTestId('pay-comment').fill('часть оплатили наличными')
  await page.getByTestId('pay-save').click()

  // Новый чек: «Изменение оплаты № N», итог тот же, оплата — новая.
  await expect(page).toHaveURL(/\/sales\/[0-9a-f-]+$/)
  await expect(page).not.toHaveURL(new RegExp(sale.id))
  await expect(page.getByTestId('correction-of')).toContainText(`Изменение оплаты № ${sale.number}`)
  await expect(page.getByText(money('Итого: 500 с'))).toBeVisible()
  // Исходный: «Изменена оплата → № M».
  await page.goto(`/sales/${sale.id}`)
  await expect(page.getByTestId('corrected-by')).toContainText('Изменена оплата →')
})

test('режим кассы не затронут: в кассе кнопки «Изменить оплату» нет', async ({ page }) => {
  await login(page)
  await page.goto('/')
  await expect(page.getByTestId('pay-change-button')).toHaveCount(0)
})

// ---------- Наличные ↔ QR в смене (задача 003 Н1) ----------

async function apiGet<T>(request: APIRequestContext, path: string): Promise<T> {
  const res = await request.get(`/api/v1${path}`)
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

/** Закрыть открытую смену без расхождения, деньги остались в кассе. */
async function closeOpenShift(request: APIRequestContext) {
  const cur = await apiGet<{ id: string; expected_tyiyn: number } | null>(request, '/shifts/current')
  if (!cur) return
  await apiPost(request, `/shifts/${cur.id}/close`, { op_id: crypto.randomUUID(), counted_tyiyn: Math.max(cur.expected_tyiyn, 0), comment: 'сквозной тест' })
  await apiPost(request, `/shifts/${cur.id}/handover`, { op_id: crypto.randomUUID(), amount_tyiyn: 0 })
}

/** Новая смена с одним чеком наличными 1 000 с. */
async function shiftWithCashSale(request: APIRequestContext, tag: string) {
  const cat = await apiPost<{ id: string }>(request, '/categories', { name: `Кат ↔ ${tag} ${RUN}`, kind: 'other' })
  const item = await apiPost<{ id: string }>(request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Фильтр ↔ ${tag} ${RUN}`,
    sale_price_tyiyn: 100_000,
  })
  await apiPost(request, '/receipts', {
    op_id: crypto.randomUUID(),
    paid_from: await ownerSafe(request),
    lines: [{ product_id: item.id, qty: 5, cost_tyiyn: 200_000 }],
  })
  const cashier = await apiPost<{ id: string }>(request, '/employees', { full_name: `Кассир ↔ ${tag} ${RUN}`, is_cashier: true, pin: '1234' })
  await closeOpenShift(request)
  const shift = await apiPost<{ id: string; number: number }>(request, '/shifts/open', {
    op_id: crypto.randomUUID(),
    cashier_employee_id: cashier.id,
    cashier_pin: '1234',
  })
  const sale = await apiPost<{ id: string; number: number }>(request, '/sales', {
    op_id: crypto.randomUUID(),
    sale_type: 'takeaway',
    cashier_id: cashier.id,
    lines: [{ kind: 'piece', product_id: item.id, qty: 1, unit_price_tyiyn: 100_000 }],
    payments: [{ method: 'cash', amount_tyiyn: 100_000 }],
  })
  return { shift, sale }
}

test('закрытие смены: «Наличные ↔ QR» — «Должно быть» меньше, безнал больше, без недостачи', async ({ page }) => {
  await login(page)
  // Кнопка в окне закрытия — по настройке «Смена» (по умолчанию да).
  const set = await page.request.put('/api/v1/settings/shift', { data: { print_on_close: false, pay_fix_on_close: true }, headers: DEVICE })
  expect(set.ok(), await set.text()).toBeTruthy()
  await shiftWithCashSale(page.request, 'закрытие')
  const before = await apiGet<{ expected_tyiyn: number }>(page.request, '/shifts/current')

  await page.goto('/shift')
  await page.getByRole('button', { name: 'Закрыть смену' }).click()
  const dialog = page.getByRole('dialog', { name: 'Закрыть смену' })
  const expectedText = dialog.getByText(/Должно быть в кассе/)
  const was = await expectedText.textContent()
  await dialog.getByTestId('cash-to-bank-button').click()
  const box = page.getByTestId('cash-to-bank')
  await expect(page.getByTestId('cash-to-bank-limit')).toContainText(money('можно до 1 000 с'))
  await page.getByTestId('cash-to-bank-amount').fill('1000')
  await page.getByTestId('cash-to-bank-comment').fill('клиент платил по QR')
  await expect(page.getByTestId('pay-effect')).toContainText('должно быть')
  await box.getByTestId('cash-to-bank-save').click()
  await expect(page.getByTestId('cash-to-bank')).toHaveCount(0)

  // Окно закрытия перечитало смену: «Должно быть» меньше на 1 000, в безнале — «Из наличных в QR».
  await expect.poll(async () => expectedText.textContent()).not.toBe(was)
  await expect(dialog.getByTestId('close-noncash')).toContainText('Из наличных в QR')
  const after = await apiGet<{ id: string; expected_tyiyn: number }>(page.request, '/shifts/current')
  expect(after.expected_tyiyn).toBe(before.expected_tyiyn - 100_000)
  await dialog.getByRole('button', { name: 'Закрыть', exact: true }).last().click()
  await page.getByRole('button', { name: 'Деньги остались в кассе' }).click()
  await expect(page.getByText('Отмечено: деньги остались в кассе')).toBeVisible()
  const report = await apiGet<{ cash: { diff_tyiyn: number | null } }>(page.request, `/shifts/${after.id}/report`)
  expect(report.cash.diff_tyiyn).toBe(0)
})

test('отчёт смены: «По чекам» ⇄ в QR, «Из QR в наличные» и «Оплата» у строки продаж', async ({ page }) => {
  await login(page)
  const { shift, sale } = await shiftWithCashSale(page.request, 'отчёт')
  await page.goto(`/shifts/${shift.id}`)
  await page.getByTestId('cash-to-bank-button').click()

  // «По чекам»: «⇄ в QR» у чека наличными — «Что изменится» от сервера, причина, «Сохранить».
  await page.getByTestId('cash-to-bank-tab-sales').click()
  const row = page.getByTestId('shift-sale-row').filter({ hasText: `№ ${sale.number}` })
  await row.getByTestId('shift-sale-swap').click()
  await expect(page.getByTestId('shift-sale-swap-modal').getByTestId('pay-effect')).toContainText('Что изменится')
  await page.getByTestId('shift-sale-swap-comment').fill('платил QR')
  await page.getByTestId('shift-sale-swap-save').click()
  await expect(page.getByTestId('shift-sale-swap-modal')).toHaveCount(0)
  // Исходный чек больше не меняется; новый — картой, у него «⇄ в наличные».
  await expect(row).not.toContainText('⇄')
  await expect(page.getByTestId('shift-sale-swap').first()).toHaveText('⇄ в наличные')

  // «Одной суммой» → «Из QR в наличные» 400 с.
  await page.getByTestId('cash-to-bank-tab-sum').click()
  await page.getByTestId('cash-to-bank-to_cash').click()
  await expect(page.getByTestId('cash-to-bank-limit')).toContainText(money('можно до 1 000 с'))
  await page.getByTestId('cash-to-bank-amount').fill('400')
  await page.getByTestId('cash-to-bank-comment').fill('часть наличными')
  await expect(page.getByTestId('pay-effect')).toContainText('больше')
  await page.getByTestId('cash-to-bank-save').click()

  // Отчёт перечитан: перенос в списке со стрелкой, строки движения денег с расшифровкой по чеку.
  await expect(page.getByTestId('transfers')).toContainText('из QR в наличные')
  await expect(page.getByTestId('flow-bank_to_cash')).toContainText(money('400 с'))
  await expect(page.getByTestId('flow-fix-item').filter({ hasText: `№ ${sale.number}` })).toBeVisible()

  // «Продажи» → «Оплата» у строки: окно «Изменить оплату» прямо в отчёте, после сохранения — тот же отчёт.
  const report = await apiGet<{ sales: { id: string; kind: string; number: number }[] }>(page.request, `/shifts/${shift.id}/report`)
  const fixed = report.sales.find((x) => x.kind === 'sale' && x.id !== sale.id)
  expect(fixed).toBeTruthy()
  await page.getByRole('tab', { name: /Продажи/ }).click()
  await page
    .getByRole('row')
    .filter({ hasText: `№ ${fixed?.number}` })
    .getByTestId('sale-pay-fix')
    .click()
  await expect(page.getByTestId('pay-change')).toBeVisible()
  await page.getByTestId('pay-card').fill('')
  await page.getByTestId('pay-cash').fill('1000')
  await page.getByTestId('pay-comment').fill('всё же наличными')
  await page.getByTestId('pay-save').click()
  await expect(page.getByTestId('pay-change')).toHaveCount(0)
  await expect(page).toHaveURL(new RegExp(`/shifts/${shift.id}`))
})

test('администратор: без права кнопки «Наличные ↔ QR» нет, с правом — есть в открытой смене', async ({ page, browser }) => {
  await login(page)
  const admin = `kassa${RUN}`
  const password = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  const user = await apiPost<{ id: string }>(page.request, '/users', { login: admin, password, full_name: `Админ ↔ ${RUN}`, role: 'admin' })
  await shiftWithCashSale(page.request, 'админ')

  const as = async () => {
    const ctx = await browser.newContext()
    const a = await ctx.newPage()
    await a.goto('/login')
    await a.getByLabel('Логин').fill(admin)
    await a.getByLabel('Пароль').fill(password)
    await a.getByRole('button', { name: 'Войти' }).click()
    await expect(a.getByRole('link', { name: 'Касса' })).toBeVisible()
    await a.goto('/shift')
    await expect(a.getByRole('button', { name: 'Закрыть смену' })).toBeVisible()
    return { ctx, a }
  }
  {
    const { ctx, a } = await as()
    await expect(a.getByTestId('cash-to-bank-button')).toHaveCount(0)
    await ctx.close()
  }
  const users = await apiGet<{ id: string; permissions?: string[] }[]>(page.request, '/users')
  const current = users.find((u) => u.id === user.id)?.permissions ?? []
  const put = await page.request.put(`/api/v1/users/${user.id}/permissions`, {
    data: { op_id: crypto.randomUUID(), permissions: [...current, 'till_pay_fix'] },
    headers: DEVICE,
  })
  expect(put.ok(), await put.text()).toBeTruthy()
  const { ctx, a } = await as()
  await a.getByTestId('cash-to-bank-button').click()
  // Администратору — без прибыли в «Что изменится».
  await a.getByTestId('cash-to-bank-amount').fill('100')
  await a.getByTestId('cash-to-bank-comment').fill('QR')
  await expect(a.getByTestId('pay-effect')).toContainText('Что изменится')
  await expect(a.getByTestId('pay-effect')).not.toContainText('Прибыль')
  await ctx.close()
})
