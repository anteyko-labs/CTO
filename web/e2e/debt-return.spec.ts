// Возврат долгового чека (SPEC-36 §8): окно само делит сумму — сначала с долга, не больше долга по
// чеку и долга клиента сейчас, остаток деньгами тем способом, которым платили; обе суммы правятся.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e361' }
const RUN = Date.now().toString().slice(-8)
/** Сумма как на экране: пробелы в разрядах и перед «с» неразрывные. */
const money = (text: string) => new RegExp(text.replace(/ /g, '\\s'))

interface SaleOut {
  id: string
  number: number
  payments: { method: string; amount_tyiyn: number }[]
  returns: { id: string }[]
}

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

/** Клиент и чек на 1 000 с с заданными оплатами; смена открыта — наличные возвращаются только в смену. */
async function debtSale(page: Page, tag: string, payments: { method: string; amount_tyiyn: number }[]) {
  const request = page.request
  const cashier = await apiPost<{ id: string }>(request, '/employees', { full_name: `Кассир ${tag} ${RUN}`, is_cashier: true, pin: '1234' })
  const cat = await apiPost<{ id: string }>(request, '/categories', { name: `${tag} ${RUN}`, kind: 'other' })
  const p = await apiPost<{ id: string }>(request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `${tag} ${RUN} фильтр`,
    article: '',
    sale_price_tyiyn: 100_000,
    min_stock: 0,
  })
  await apiPost(request, '/receipts', {
    op_id: crypto.randomUUID(),
    paid_from: await ownerSafe(request),
    lines: [{ product_id: p.id, qty: 2, cost_tyiyn: 120_000 }],
  })
  const party = await apiPost<{ id: string }>(request, '/parties', { role: 'customer', kind: 'person', name: `${tag} Клиент ${RUN}`, inn: `5${RUN}12345` })
  const current = await request.get('/api/v1/shifts/current')
  if (!(await current.json())) await apiPost(request, '/shifts/open', { op_id: crypto.randomUUID(), cashier_employee_id: cashier.id, cashier_pin: '1234' })
  const sale = await apiPost<SaleOut>(request, '/sales', {
    op_id: crypto.randomUUID(),
    client_time: new Date().toISOString(),
    sale_type: 'takeaway',
    cashier_id: cashier.id,
    master_id: null,
    party_id: party.id,
    contact_id: null,
    vehicle_id: null,
    comment: '',
    lines: [{ kind: 'piece', gift: false, product_id: p.id, service_id: null, qty: 1, unit_price_tyiyn: 100_000 }],
    payments,
  })
  return { sale, party }
}

async function openReturn(page: Page, sale: SaleOut) {
  await page.goto(`/sales/${sale.id}`)
  await page.getByRole('button', { name: 'Возврат', exact: true }).click()
  await expect(page.getByRole('heading', { name: `Возврат по чеку № ${sale.number}` })).toBeVisible()
  await page.getByRole('row', { name: /фильтр/ }).getByRole('textbox').fill('1')
}

test('смешанный чек «600 наличными + 400 в долг» возвращается 400 с долга и 600 наличными', async ({ page }) => {
  await login(page)
  const { sale, party } = await debtSale(page, 'Смешанный', [
    { method: 'cash', amount_tyiyn: 60_000 },
    { method: 'debt', amount_tyiyn: 40_000 },
  ])
  await openReturn(page, sale)
  await expect(page.getByTestId('return-debt-hint')).toHaveText(money('Клиент должен 400 с, по этому чеку в долг 400 с'))
  await expect(page.locator('#return-debt')).toHaveValue('400')
  await expect(page.locator('#return-money')).toHaveValue('600')
  await expect(page.locator('#return-method')).toHaveValue('cash')
  await page.getByLabel('Причина').fill('не подошёл')
  await page.getByRole('button', { name: 'Оформить возврат' }).click()
  await expect(page.getByRole('heading', { name: /^Возврат № / })).toBeVisible()

  const back = await apiGet<SaleOut>(page.request, `/sales/${sale.id}`)
  const ret = await apiGet<SaleOut>(page.request, `/sales/${back.returns[0].id}`)
  expect(ret.payments.map((p) => [p.method, p.amount_tyiyn]).sort()).toEqual([
    ['cash', -60_000],
    ['debt', -40_000],
  ])
  const after = await apiGet<{ balance_tyiyn: number }>(page.request, `/parties/${party.id}`)
  expect(after.balance_tyiyn).toBe(0)
})

test('долг частично погашен: с долга — сколько клиент должен, остаток деньгами; суммы правятся', async ({ page }) => {
  await login(page)
  const { sale, party } = await debtSale(page, 'Погашенный', [{ method: 'debt', amount_tyiyn: 100_000 }])
  await apiPost(page.request, '/debts/repayments', { op_id: crypto.randomUUID(), party_id: party.id, amount_tyiyn: 70_000, method: 'cash', comment: '' })

  await openReturn(page, sale)
  await expect(page.getByTestId('return-debt-hint')).toHaveText(money('Клиент должен 300 с, по этому чеку в долг 1 000 с'))
  await expect(page.locator('#return-debt')).toHaveValue('300')
  await expect(page.locator('#return-money')).toHaveValue('700')
  // Больше, чем клиент должен, с долга не снять.
  await page.locator('#return-debt').fill('500')
  await expect(page.locator('#return-money')).toHaveValue('500')
  await page.getByLabel('Причина').fill('вернул')
  await expect(page.getByRole('button', { name: 'Оформить возврат' })).toBeDisabled()
  // Правка денег пересчитывает долг.
  await page.locator('#return-money').fill('900')
  await expect(page.locator('#return-debt')).toHaveValue('100')
  await page.getByRole('button', { name: 'Оформить возврат' }).click()
  await expect(page.getByRole('heading', { name: /^Возврат № / })).toBeVisible()

  const back = await apiGet<SaleOut>(page.request, `/sales/${sale.id}`)
  const ret = await apiGet<SaleOut>(page.request, `/sales/${back.returns[0].id}`)
  expect(ret.payments.map((p) => [p.method, p.amount_tyiyn]).sort()).toEqual([
    ['cash', -90_000],
    ['debt', -10_000],
  ])
  const after = await apiGet<{ balance_tyiyn: number }>(page.request, `/parties/${party.id}`)
  expect(after.balance_tyiyn).toBe(20_000)
})
