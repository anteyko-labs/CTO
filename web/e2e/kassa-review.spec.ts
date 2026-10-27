// Находки проверки кассы 27.10.2026: чек не правится, пока проводится, а товар, отсканированный в
// это время, встаёт в следующий чек; скан берёт товар из снимка и при 502/5xx; цена в чеке
// сверяется с каталогом перед проведением; касса открывается и без защищённого контекста.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ownerSafe } from './safe'
import { pickCashier } from './cashier'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e31a' }
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

interface Product {
  id: string
  name: string
  barcodes: string[]
}

let seq = 0

/** Товар с остатком 5 шт по цене `price` тыйын. */
async function product(request: APIRequestContext, price: number): Promise<Product> {
  const cat = await apiPost<{ id: string }>(request, '/categories', { name: `Проверка кассы ${RUN}${++seq}`, kind: 'other' })
  const p = await apiPost<Product>(request, '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name: `Товар проверки ${RUN}-${seq}`, sale_price_tyiyn: price })
  await apiPost(request, '/receipts', { op_id: crypto.randomUUID(), paid_from: await ownerSafe(request), lines: [{ product_id: p.id, qty: 5, cost_tyiyn: price / 2 }] })
  return p
}

async function scan(page: Page, code: string) {
  const picker = page.getByPlaceholder(PICKER)
  await picker.fill(code)
  await picker.press('Enter')
}

/** Снимок каталога сохранён на устройстве. */
async function snapshotSaved(page: Page) {
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          new Promise<boolean>((res) => {
            const r = indexedDB.open('avtodom', 1)
            r.onsuccess = () => {
              const g = r.result.transaction('snapshot').objectStore('snapshot').getKey('meta')
              g.onsuccess = () => res(g.result !== undefined)
              g.onerror = () => res(false)
            }
            r.onerror = () => res(false)
          }),
      ),
    )
    .toBe(true)
}

test('пока чек проводится, его не отложить и не очистить; скан встаёт в следующий чек', async ({ page }) => {
  await login(page)
  const cashier = `Кассир проверки ${RUN}`
  await apiPost(page.request, '/employees', { full_name: cashier, is_cashier: true, pin: '1234' })
  const a = await product(page.request, 50_000)
  const b = await product(page.request, 30_000)
  await page.goto('/')
  await pickCashier(page, cashier)
  await scan(page, a.barcodes[0])
  await expect(page.getByTestId('cart').getByText(a.name)).toBeVisible()
  await page.getByRole('button', { name: money('1 000 с') }).click()

  // Ответ сервера задержан: чек «в пути».
  let release: () => void = () => undefined
  const held = new Promise<void>((r) => (release = r))
  await page.route(/\/api\/v1\/sales$/, async (route) => {
    if (route.request().method() === 'POST') await held
    await route.continue()
  })
  await page.getByRole('button', { name: 'Провести чек' }).click()
  await expect(page.getByRole('button', { name: 'Отложить', exact: true })).toBeDisabled()
  await expect(page.getByRole('button', { name: 'Очистить', exact: true })).toBeDisabled()
  // Следующий покупатель: товар сканируют, пока ответа нет.
  await scan(page, b.barcodes[0])
  await expect(page.getByText(/встанет в следующий чек/)).toBeVisible()
  release()
  await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()
  // В новом чеке — только товар следующего покупателя, отложенных копий проданного нет.
  const cart = page.getByTestId('cart')
  await expect(cart.getByText(b.name)).toBeVisible()
  await expect(cart.getByText(a.name)).toHaveCount(0)
  await expect(page.getByText('Отложенные:')).toHaveCount(0)
  await page.unroute(/\/api\/v1\/sales$/)
})

test('скан при 502 от прокси берёт товар из снимка', async ({ page }) => {
  await login(page)
  const p = await product(page.request, 20_000)
  await page.goto('/')
  await snapshotSaved(page)
  await page.waitForLoadState('networkidle')
  // Снимок с товаром — свежий: перечитываем кассу, чтобы он точно был в снимке.
  await page.reload()
  await snapshotSaved(page)
  await page.waitForLoadState('networkidle')
  await page.route(/\/api\/v1\/products\/by-barcode\//, (route) => route.fulfill({ status: 502, contentType: 'text/html', body: '<html>Bad Gateway</html>' }))
  await scan(page, p.barcodes[0])
  await expect(page.getByTestId('cart').getByText(p.name)).toBeVisible()
  await page.unroute(/\/api\/v1\/products\/by-barcode\//)
})

test('цена сменилась, пока товар лежал в чеке, — касса берёт новую и предупреждает', async ({ page }) => {
  await login(page)
  const cashier = `Кассир цены ${RUN}`
  await apiPost(page.request, '/employees', { full_name: cashier, is_cashier: true, pin: '1234' })
  const p = await product(page.request, 40_000)
  await page.goto('/')
  await pickCashier(page, cashier)
  await scan(page, p.barcodes[0])
  await expect(page.getByTestId('cart').getByText(p.name)).toBeVisible()
  // Владелец снизил цену, касса об этом не знает.
  const res = await page.request.patch(`/api/v1/products/${p.id}/prices`, { data: { sale_price_tyiyn: 35_000 }, headers: DEVICE })
  expect(res.ok(), await res.text()).toBeTruthy()
  await page.getByRole('button', { name: money('500 с') }).click()
  await page.getByRole('button', { name: 'Провести чек' }).click()
  // Чек не проведён по старой цене: строка — по новой, кассир видит, что поменялось.
  await expect(page.getByTestId('price-changed')).toContainText(money('было 400 с, стало 350 с'))
  await expect(page.getByText(/Чек № \d+ проведён/)).toHaveCount(0)
  await expect(page.getByTestId('cart')).toContainText(money('350 с'))
  await page.getByRole('button', { name: 'Провести чек' }).click()
  await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()
  const link = page.getByRole('link', { name: 'Открыть', exact: true })
  const href = await link.getAttribute('href')
  const sale = (await (await page.request.get(`/api/v1${href ?? ''}`)).json()) as { lines: { unit_price_tyiyn: number }[] }
  expect(sale.lines[0].unit_price_tyiyn).toBe(35_000)
})

test('отложенный чек после смены цены возвращается с новой ценой', async ({ page }) => {
  await login(page)
  const p = await product(page.request, 40_000)
  await page.goto('/')
  await scan(page, p.barcodes[0])
  await expect(page.getByTestId('cart').getByText(p.name)).toBeVisible()
  await page.getByRole('button', { name: 'Отложить', exact: true }).click()
  await expect(page.getByText('Отложенные:')).toBeVisible()
  const res = await page.request.patch(`/api/v1/products/${p.id}/prices`, { data: { sale_price_tyiyn: 45_000 }, headers: DEVICE })
  expect(res.ok(), await res.text()).toBeTruthy()
  await page.getByRole('button', { name: /Чек на 1 поз\./ }).last().click()
  await expect(page.getByTestId('price-changed')).toContainText(money('было 400 с, стало 450 с'))
  await expect(page.getByTestId('cart')).toContainText(money('450 с'))
  // Чек — дальше по новой цене; отложенные — после него чистые.
  await page.getByRole('button', { name: 'Очистить', exact: true }).click()
  await page.getByRole('button', { name: 'Точно очистить?' }).click()
})

test('касса открывается без crypto.randomUUID (простой HTTP не с localhost)', async ({ page }) => {
  const errors: string[] = []
  page.on('pageerror', (e) => errors.push(e.message))
  await page.addInitScript(() => {
    Object.defineProperty(Crypto.prototype, 'randomUUID', { value: undefined, configurable: true })
    localStorage.removeItem('avtodom.device_id')
  })
  await login(page)
  const id = await page.evaluate(() => localStorage.getItem('avtodom.device_id'))
  expect(id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-[47][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/)
  await page.getByRole('link', { name: 'Касса' }).click()
  await expect(page.getByPlaceholder(PICKER)).toBeVisible()
  expect(errors).toEqual([])
})
