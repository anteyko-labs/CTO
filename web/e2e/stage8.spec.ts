// Доработки по проверке на точке в браузере (SPEC-27 §4–5): список товаров с итогами, «+ Товар»
// из категории, цена продажи и прошлая закупка в приходе, частичная оплата накладной.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2e8' }
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

interface Created {
  id: string
  barcodes: string[]
}

async function product(request: APIRequestContext, categoryId: string, name: string, price: number): Promise<Created> {
  return apiPost<Created>(request, '/products', { op_id: crypto.randomUUID(), category_id: categoryId, name, sale_price_tyiyn: price })
}

test('список товаров: итоги по отбору, сортировка и «+ Товар» из категории', async ({ page }) => {
  await login(page)
  const category = `Свечи ${RUN}`
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: category, kind: 'other' })
  const a = await product(page.request, cat.id, `Свеча А ${RUN}`, 15_000)
  const b = await product(page.request, cat.id, `Свеча Б ${RUN}`, 25_000)
  // А: 3 шт по 100 с (продажа 150 с), Б: 2 шт по 200 с (продажа 250 с).
  await apiPost(page.request, '/receipts', {
    op_id: crypto.randomUUID(),
    paid_from: await ownerSafe(page.request),
    lines: [
      { product_id: a.id, qty: 3, cost_tyiyn: 30_000 },
      { product_id: b.id, qty: 2, cost_tyiyn: 40_000 },
    ],
  })

  await page.goto('/products')
  await page.getByLabel('Категория').selectOption({ label: category })
  const table = page.getByRole('table', { name: 'Список товаров' })
  await expect(table.getByRole('row')).toHaveCount(3)
  const totals = page.getByTestId('product-totals')
  // Итоги — сумма строк: по закупке 300 + 400 с, по продаже 450 + 500 с.
  await expect(totals).toContainText('Итого по отбору: 2 товара')
  await expect(totals).toContainText(money('по закупке 700 с'))
  await expect(totals).toContainText(money('по продаже 950 с'))
  const rowA = table.getByRole('row', { name: new RegExp(`Свеча А ${RUN}`) })
  await expect(rowA).toContainText(money('100 с'))
  await expect(rowA).toContainText(money('450 с'))
  // Сортировка по сумме по продаже: сначала большие — Б.
  await table.getByRole('button', { name: 'Сумма по продаже' }).click()
  await expect(table.getByRole('row').nth(1)).toContainText(`Свеча Б ${RUN}`)
  await table.getByRole('button', { name: 'Сумма по продаже' }).click()
  await expect(table.getByRole('row').nth(1)).toContainText(`Свеча А ${RUN}`)

  // «+ Товар» на экране категорий: форма с уже выбранной категорией.
  await page.goto('/categories')
  await page.getByRole('button', { name: `Новый товар в категории ${category}` }).click()
  await expect(page.getByRole('heading', { name: 'Новый товар' })).toBeVisible()
  await expect(page.getByLabel('Категория')).toHaveValue(cat.id)
})

test('приход: цена продажи и прошлая закупка; частичная оплата накладной', async ({ page }) => {
  await login(page)
  const supplierName = `Автоснаб ${RUN}`
  const supplier = await apiPost<{ id: string }>(page.request, '/suppliers', { name: supplierName })
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Колодки ${RUN}`, kind: 'other' })
  const pads = await product(page.request, cat.id, `Колодки ${RUN}`, 50_000)
  // Накладная в долг: 4 шт по 250 с = 1 000 с.
  const receipt = await apiPost<{ id: string; number: number }>(page.request, '/receipts', {
    op_id: crypto.randomUUID(),
    supplier_id: supplier.id,
    payment: 'debt',
    lines: [{ product_id: pads.id, qty: 4, cost_tyiyn: 100_000 }],
  })

  // Новый приход: в строке видна цена продажи и прошлая закупка, внизу — итог по продаже.
  await page.goto('/receipts/new')
  const picker = page.getByPlaceholder(PICKER)
  await picker.fill(pads.barcodes[0])
  await picker.press('Enter')
  await expect(page.getByTestId('last-receipt')).toContainText(money(`в прошлый раз: 250 с, `))
  await expect(page.getByTestId('last-receipt')).toContainText(supplierName)
  await page.getByLabel('Кол-во, шт').fill('2')
  await expect(page.getByTestId('receipt-sale-total')).toHaveText(money('1 000 с'))

  // Карточка поставщика: накладная не оплачена, платим 400 с из 1 000 с.
  await page.goto(`/suppliers/${supplier.id}`)
  await page.getByRole('tab', { name: 'Накладные' }).click()
  const row = page.getByRole('row', { name: new RegExp(`№ ${receipt.number}\\b`) })
  await expect(row).toContainText('не оплачена')
  await row.getByRole('button', { name: 'Оплатить' }).click()
  const dialog = page.getByRole('dialog', { name: new RegExp(`Оплата накладной № ${receipt.number}`) })
  await expect(dialog).toBeVisible()
  await dialog.getByLabel('Сумма, с').fill('400')
  // «Вне кассы» больше нет (ADR-079): владелец платит переводом со «Счёта».
  await dialog.getByRole('button', { name: 'QR владельцу' }).click()
  await dialog.getByRole('button', { name: 'Оплатить' }).click()
  // Окно показывает итог оплаты (SPEC-28 §3) и закрывается кнопкой «Готово».
  await expect(dialog.getByTestId('repay-result')).toContainText(money('400 с'))
  await dialog.getByRole('button', { name: 'Готово' }).click()
  await expect(dialog).toBeHidden()
  await expect(row).toContainText('частично')
  await expect(row).toContainText(money('600 с'))

  // История платежей: кто, сколько, откуда и какую накладную закрыло.
  await page.getByRole('tab', { name: 'Платежи' }).click()
  const pay = page.getByRole('row', { name: new RegExp(`№ ${receipt.number}`) })
  await expect(pay).toContainText(money('400 с'))
  await expect(pay).toContainText('Счёт')

  // В накладной — оплачено и осталось, платёж в истории.
  await pay.getByRole('link', { name: `№ ${receipt.number}` }).click()
  await expect(page.getByTestId('invoice-paid')).toHaveText(money('400 с'))
  await expect(page.getByTestId('invoice-left')).toHaveText(money('600 с'))
  await expect(page.getByText('частично').first()).toBeVisible()
})
