// Строка прихода как в старой программе (SPEC-28 §1): цена по накладной, наценка %, цена продажи,
// сумма. Наценка и цена продажи связаны; новая цена ставится товару при проведении прихода,
// владелец видит смену цены в уведомлениях. «Чаще берут» / «Больше приносят» (SPEC-28 §4).
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2ea' }
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

test('приход: наценка и цена продажи связаны, новая цена ставится товару', async ({ page }) => {
  await login(page)
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Наценка ${RUN}`, kind: 'other' })
  const name = `Ремень ${RUN}`
  const p = await apiPost<{ id: string; barcodes: string[] }>(page.request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name,
    sale_price_tyiyn: 180_000,
  })

  await page.goto('/receipts/new')
  const picker = page.getByPlaceholder(PICKER)
  await picker.fill(p.barcodes[0])
  await picker.press('Enter')
  // По умолчанию цена продажи — текущая цена товара.
  const sale = page.getByLabel('Цена продажи')
  const markup = page.getByLabel('Наценка, %')
  await expect(sale).toHaveValue('1800')
  await page.getByLabel('Кол-во, шт').fill('10')
  await page.getByLabel('Цена за шт, с').fill('1600')
  await expect(markup).toHaveValue('12,5')
  // Наценка → цена: 25 % от 1 600 с = 2 000 с.
  await markup.fill('25')
  await expect(sale).toHaveValue('2000')
  // Цена → наценка: 2 100 с — 31,3 %.
  await sale.fill('2100')
  await expect(markup).toHaveValue('31,3')
  await expect(page.getByTestId('new-sale-price')).toContainText(money('было 1 800 с'))
  await expect(page.getByTestId('receipt-sale-total')).toHaveText(money('21 000 с'))
  await page.locator('#receipt-paid-from').selectOption('outside')
  await page.getByRole('button', { name: 'Провести приход' }).first().click()
  await expect(page).toHaveURL(/\/receipts\/[0-9a-f-]{36}$/)
  // В накладной — цена по накладной, наценка и цена продажи.
  const row = page.getByRole('row', { name: new RegExp(name) })
  await expect(row).toContainText(money('1 600 с'))
  await expect(row).toContainText(money('31,3 %'))
  await expect(row).toContainText(money('2 100 с'))

  // Цена товара поменялась; владелец видит это в уведомлениях с номером прихода.
  const res = await page.request.get(`/api/v1/products/${p.id}`, { headers: DEVICE })
  expect(((await res.json()) as { sale_price_tyiyn: number }).sale_price_tyiyn).toBe(210_000)
  await page.goto('/notifications')
  await expect(page.getByText(`Изменена цена: ${name}`).first()).toBeVisible()
  await expect(page.getByText(/приход № \d+, наценка 31,3/).first()).toBeVisible()

  // «Продаваемые»: у владельца две вкладки.
  await page.goto('/top-products')
  await expect(page.getByRole('tab', { name: 'Чаще берут' })).toBeVisible()
  await page.getByRole('tab', { name: 'Больше приносят' }).click()
  await expect(page.getByRole('tab', { name: 'Больше приносят' })).toHaveAttribute('aria-selected', 'true')
})
