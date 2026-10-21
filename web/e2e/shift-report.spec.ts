// Отчёт смены, как в Умаге, и отменённые товары (SPEC-30 §7, §8): строка, удалённая крестиком,
// уходит с чеком, «Очистить» — отдельной записью; отчёт открывается из «Смены», вкладки
// показывают документы смены.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e301' }
const RUN = Date.now().toString().slice(-8)
const PICKER = /Сканируйте штрихкод/
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

async function apiGet<T>(request: APIRequestContext, path: string): Promise<T> {
  const res = await request.get(`/api/v1${path}`)
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

/** Закрыть открытую смену без расхождения и отметить, что деньги остались в кассе. */
async function closeOpenShift(request: APIRequestContext) {
  const cur = await apiGet<{ id: string; expected_tyiyn: number } | null>(request, '/shifts/current')
  if (!cur) return
  await apiPost(request, `/shifts/${cur.id}/close`, { op_id: crypto.randomUUID(), counted_tyiyn: Math.max(cur.expected_tyiyn, 0), comment: 'сквозной тест' })
  await apiPost(request, `/shifts/${cur.id}/handover`, { op_id: crypto.randomUUID(), amount_tyiyn: 0 })
}

async function scan(page: Page, code: string) {
  const picker = page.getByPlaceholder(PICKER)
  await picker.fill(code)
  await picker.press('Enter')
}

test('отчёт смены: движение денег, вкладки и отменённые товары', async ({ page }) => {
  await login(page)
  const cashier = `Кассир отчёта ${RUN}`
  const filter = `Фильтр отчёта ${RUN}`
  const oil = `Присадка отчёта ${RUN}`
  const codeA = `27${RUN}101`
  const codeB = `27${RUN}102`
  const emp = await apiPost<{ id: string }>(page.request, '/employees', { full_name: cashier, is_cashier: true, pin: '1234' })
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Отчёт ${RUN}`, kind: 'filter' })
  const a = await apiPost<{ id: string }>(page.request, '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name: filter, sale_price_tyiyn: 50_000, barcodes: [codeA] })
  const b = await apiPost<{ id: string }>(page.request, '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name: oil, sale_price_tyiyn: 30_000, barcodes: [codeB] })
  await apiPost(page.request, '/receipts', {
    op_id: crypto.randomUUID(),
    payment: 'paid',
    paid_from: await ownerSafe(page.request),
    lines: [
      { product_id: a.id, qty: 10, cost_tyiyn: 200_000 },
      { product_id: b.id, qty: 10, cost_tyiyn: 100_000 },
    ],
  })
  await closeOpenShift(page.request)
  const shift = await apiPost<{ id: string; number: number }>(page.request, '/shifts/open', {
    op_id: crypto.randomUUID(),
    cashier_employee_id: emp.id,
    cashier_pin: '1234',
  })
  await apiPost(page.request, '/cash/movements', { op_id: crypto.randomUUID(), kind: 'cash_in', amount_tyiyn: 100_000, comment: `размен ${RUN}` })

  // Касса: две строки, присадку убрали крестиком, фильтр продали.
  await page.goto('/')
  await scan(page, codeA)
  await expect(page.getByTestId('cart').getByText(filter)).toBeVisible()
  await scan(page, codeB)
  const cart = page.getByTestId('cart')
  await expect(cart.getByText(oil)).toBeVisible()
  await cart.locator('li', { hasText: oil }).getByRole('button', { name: 'Удалить строку' }).click()
  await expect(cart.getByText(oil)).toHaveCount(0)
  await page.getByRole('button', { name: 'Провести чек' }).first().click()
  await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()

  // «Очистить»: строка уходит отдельной записью через очередь устройства.
  await scan(page, codeB)
  await expect(cart.getByText(oil)).toBeVisible()
  await page.getByRole('button', { name: 'Очистить', exact: true }).click()
  await page.getByRole('button', { name: 'Точно очистить?' }).click()
  await expect(page.getByText('Чек пуст')).toBeVisible()
  await expect
    .poll(async () => (await apiGet<{ cancelled: unknown[] }>(page.request, `/shifts/${shift.id}/report`)).cancelled.length, { timeout: 40_000 })
    .toBe(2)

  // Отчёт открывается из «Смены».
  await page.goto('/shift')
  await page.getByRole('link', { name: 'Отчёт смены' }).click()
  await expect(page).toHaveURL(new RegExp(`/shifts/${shift.id}$`))
  await expect(page.getByRole('heading', { name: `Смена № ${shift.number}` })).toBeVisible()
  await expect(page.getByText(`Кассир: ${cashier}`)).toBeVisible()

  // Обзор: на начало + приход − расход = должно быть; выручка 500 с, наличными.
  const flow = page.getByTestId('cash-flow')
  await expect(flow.getByTestId('flow-cash_in')).toContainText(money('1 000 с'))
  await expect(flow.getByTestId('flow-cash_sales')).toContainText(money('500 с'))
  const report = await apiGet<{ cash: { opening_tyiyn: number; income_tyiyn: number; outgo_tyiyn: number; expected_tyiyn: number } }>(page.request, `/shifts/${shift.id}/report`)
  expect(report.cash.opening_tyiyn + report.cash.income_tyiyn - report.cash.outgo_tyiyn).toBe(report.cash.expected_tyiyn)
  await expect(page.getByTestId('revenue-net')).toContainText(money('500 с'))
  await expect(page.getByTestId('method-cash')).toContainText(money('500 с'))
  // Владельцу — валовая прибыль: 500 − 200 = 300 с.
  await expect(page.getByTestId('gross')).toContainText(money('300 с'))
  await expect(page.getByTestId('hour-bars')).toBeVisible()

  // Продажи: чек смены.
  await page.getByRole('tab', { name: /Продажи/ }).click()
  await expect(page.getByRole('cell', { name: 'Наличные' }).first()).toBeVisible()
  await page.getByTestId('sales-search').fill('999999999')
  await expect(page.getByText('Ничего не нашлось')).toBeVisible()
  await page.getByTestId('sales-search').fill('')

  // Денежные операции: внесение с комментарием.
  await page.getByRole('tab', { name: /Денежные операции/ }).click()
  await expect(page.getByTestId('money-row').filter({ hasText: `размен ${RUN}` })).toContainText('Внесение')
  await expect(page.getByTestId('money-total-cash_in')).toContainText(money('1 000 с'))

  // Отменённые товары: удалённая крестиком — с номером проведённого чека, «Очистить» — без чека.
  await page.getByRole('tab', { name: /Отменённые товары/ }).click()
  const rows = page.getByTestId('cancelled-row')
  await expect(rows).toHaveCount(2)
  await expect(rows.filter({ hasText: 'Удалена из чека' })).toContainText(oil)
  await expect(rows.filter({ hasText: 'Удалена из чека' })).toContainText(/проведён № \d+/)
  await expect(rows.filter({ hasText: '«Очистить»' })).toContainText('не проведён')
  await expect(page.getByTestId('cancelled-total')).toContainText(money('600 с'))
  await expect(page).toHaveURL(/tab=cancelled/)

  await closeOpenShift(page.request)
})
