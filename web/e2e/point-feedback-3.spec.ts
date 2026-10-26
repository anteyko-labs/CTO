// Доработки по проверке на точке, часть 3 (SPEC-29): приход с клавиатуры и подсказкой из старой
// программы, часть оплаты сразу и фото накладной; оприходование без поставщика; возврат
// поставщику; товар не из программы в ревизии.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2f3' }
const RUN = Date.now().toString().slice(-8)
const money = (text: string) => new RegExp(text.replace(/ /g, '\\s'))
/** Снимок 1×1 точка — «фото накладной». */
const PNG = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==', 'base64')

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

async function product(request: APIRequestContext, category: string, name: string, code: string, price: number) {
  return apiPost<{ id: string }>(request, '/products', { op_id: crypto.randomUUID(), category_id: category, name, barcodes: [code], sale_price_tyiyn: price })
}

test('приход: подсказка из старой программы, ввод с клавиатуры, часть оплаты сразу, фото накладной', async ({ page }) => {
  await login(page)
  const catName = `Свечи ${RUN}`
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: catName, kind: 'other' })
  const supplier = await apiPost<{ id: string; name: string }>(page.request, '/suppliers', { name: `Поставщик свечей ${RUN}` })
  const code = `47${RUN}101`
  await product(page.request, cat.id, `Колпачок ${RUN}`, code, 15_000)
  // Список старой программы: позиции «Свеча NGK» у нас ещё нет. Артикул — свой у каждого прогона:
  // артикул-код Умага переходит к заведённому товару (SPEC-35), и строка с тем же артикулом в
  // следующем прогоне считалась бы уже заведённой.
  const res = await page.request.put('/api/v1/old-catalog', {
    data: [{ name: `Свеча NGK ${RUN}`, barcode: `48${RUN}202`, article: `BKR6E-${RUN}`, cost_tyiyn: 20_000, price_tyiyn: 30_000, unit: 'шт', category: catName }],
    headers: DEVICE,
  })
  expect(res.ok()).toBeTruthy()
  const accounts = await apiGet<{ id: string; kind: string; name: string }[]>(page.request, '/cash/accounts')
  const bank = accounts.find((a) => a.kind === 'bank' && a.name === 'Счёт') ?? accounts.find((a) => a.kind === 'bank')

  await page.goto('/receipts/new')
  await page.locator('#receipt-supplier').selectOption(supplier.id)
  const picker = page.locator('#receipt-picker')
  await picker.fill(`Свеча NGK ${RUN}`)
  await page.locator('[data-old-hit]').first().click()
  // Карточка из старой программы: название, категория, артикул и цена — оттуда.
  const card = page.getByRole('dialog', { name: 'Новый товар' })
  await expect(card.getByText('Данные взяты из старой программы')).toBeVisible()
  await expect(card.locator('#product-name')).toHaveValue(`Свеча NGK ${RUN}`)
  await expect(card.locator('#product-price')).toHaveValue('300')
  await card.getByRole('button', { name: 'Сохранить' }).click()
  await expect(card).toBeHidden()

  // Курсор — в количестве новой строки; Enter ведёт по полям и возвращает в поиск.
  const count = page.locator('#line-1-count')
  await expect(count).toBeFocused()
  await page.keyboard.type('4')
  await page.keyboard.press('Enter')
  await expect(page.locator('#line-1-price')).toBeFocused()
  await expect(page.locator('#line-1-price')).toHaveValue('200')
  await page.keyboard.press('Enter')
  await expect(page.locator('#line-1-markup')).toBeFocused()
  await page.keyboard.press('Enter')
  await expect(page.locator('#line-1-sale')).toBeFocused()
  await page.keyboard.press('Enter')
  await expect(picker).toBeFocused()
  // Скан — строка добавляется, повторный скан прибавляет количество, курсор остаётся в поиске.
  await picker.fill(code)
  await picker.press('Enter')
  await expect(page.locator('#line-2-count')).toHaveValue('1')
  await expect(picker).toBeFocused()
  await picker.fill(code)
  await picker.press('Enter')
  await expect(page.locator('#line-2-count')).toHaveValue('2')
  await page.locator('#line-2-price').fill('100')
  await expect(page.getByText(money('Итого: 1 000 с'))).toBeVisible()

  // Часть сейчас со «Счёта», остаток — в долг поставщику.
  await page.getByLabel('Часть сейчас, остаток в долг').check()
  await page.locator('#receipt-paid-now').fill('400')
  await expect(page.getByTestId('receipt-debt-rest')).toHaveText(money('в долг 600 с'))
  await page.locator('#receipt-paid-from').selectOption(bank!.id)
  // Фото накладной: ждёт проведения на экране.
  await page.getByTestId('receipt-photo-input').setInputFiles({ name: 'nakladnaya.png', mimeType: 'image/png', buffer: PNG })
  await expect(page.getByTestId('pending-photos').locator('li')).toHaveCount(1)
  const print = page.getByLabel('Печатать этикетки')
  if (await print.isChecked()) await print.uncheck()
  await page.getByRole('button', { name: 'Провести приход' }).first().click()
  await expect(page).toHaveURL(/\/receipts\/[0-9a-f-]{36}$/)
  await expect(page.getByTestId('invoice-paid')).toHaveText(money('400 с'))
  await expect(page.getByTestId('invoice-left')).toHaveText(money('600 с'))
  await expect(page.getByText('частично').first()).toBeVisible()
  await expect(page.getByTestId('receipt-photos').locator('img')).toHaveCount(1)
  const id = page.url().split('/').pop()!
  const files = await apiGet<{ content_type: string }[]>(page.request, `/receipts/${id}/files`)
  expect(files.map((f) => f.content_type)).toEqual(['image/jpeg'])
  // Догрузить фото можно и после проведения.
  await page.getByTestId('receipt-photo-input').setInputFiles({ name: 'page2.png', mimeType: 'image/png', buffer: PNG })
  await expect(page.getByTestId('receipt-photos').locator('img')).toHaveCount(2)
})

test('оприходование без поставщика: из строки товара — экран прихода, комментарий; сторно', async ({ page }) => {
  await login(page)
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Найдено ${RUN}`, kind: 'other' })
  const name = `Хомут ${RUN}`
  const p = await product(page.request, cat.id, name, `49${RUN}303`, 10_000)
  await page.goto('/products')
  await page.getByLabel('Поиск', { exact: true }).fill(name)
  const row = page.getByRole('row', { name: new RegExp(name) })
  await expect(row).toBeVisible()
  await row.getByRole('button', { name: 'Ещё действия' }).click()
  await page.getByRole('menuitem', { name: '+ Оприходовать' }).click()
  // «Новый приход» в виде оприходования, товар — первой строкой.
  await expect(page.getByRole('radio', { name: 'Оприходование (без поставщика)' })).toHaveAttribute('aria-checked', 'true')
  await expect(page.getByText(`1. ${name}`)).toBeVisible()
  await page.locator('#line-1-count').fill('3')
  await page.locator('#line-1-price').fill('50')
  const submit = page.getByRole('button', { name: 'Провести оприходование' })
  await expect(submit).toBeDisabled()
  await page.locator('#receipt-comment').fill('нашли на складе')
  await expect(page.getByTestId('receipt-total')).toHaveText(money('150 с'))
  await submit.click()
  await expect(page.getByRole('heading', { name: /^Оприходование № \d+$/ })).toBeVisible()
  await expect(page.getByTestId('stock-in-comment')).toHaveText('нашли на складе')
  let stock = await apiGet<{ stock_qty: number }>(page.request, `/products/${p.id}`)
  expect(stock.stock_qty).toBe(3)
  // Сторно — с причиной; товар уходит со склада.
  await page.getByRole('button', { name: 'Сторно' }).click()
  await page.getByLabel('Причина').fill('ошиблись')
  await page.getByRole('button', { name: 'Сторнировать' }).click()
  // Окно сторно закрылось — открыт документ сторно (у окна тот же заголовок).
  await expect(page.getByRole('dialog')).toHaveCount(0)
  await expect(page.getByRole('heading', { name: /^Сторно оприходования № \d+$/ })).toBeVisible()
  stock = await apiGet<{ stock_qty: number }>(page.request, `/products/${p.id}`)
  expect(stock.stock_qty).toBe(0)
  // Склад сходится с движениями.
  expect(await apiGet<unknown[]>(page.request, '/stock/verify')).toEqual([])
})

test('возврат поставщику в счёт долга: товар со склада, долг меньше, в карточке «Вернули»', async ({ page }) => {
  await login(page)
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Возврат ${RUN}`, kind: 'other' })
  const supplier = await apiPost<{ id: string }>(page.request, '/suppliers', { name: `Возвратный ${RUN}` })
  const code = `50${RUN}404`
  const p = await product(page.request, cat.id, `Лампа ${RUN}`, code, 20_000)
  await apiPost(page.request, '/receipts', {
    op_id: crypto.randomUUID(),
    supplier_id: supplier.id,
    payment: 'debt',
    lines: [{ product_id: p.id, qty: 10, cost_tyiyn: 100_000 }],
  })

  await page.goto(`/suppliers/${supplier.id}`)
  await page.getByRole('button', { name: 'Возврат поставщику' }).click()
  const dialog = page.getByRole('dialog', { name: new RegExp('Возврат поставщику') })
  const picker = dialog.getByPlaceholder(/товар, который возвращаем/)
  await picker.fill(code)
  await picker.press('Enter')
  await dialog.locator('#return-qty-1').fill('4')
  // Сумма по цене последнего прихода от него: 4 × 100 с.
  await expect(dialog.locator('#return-amount-1')).toHaveValue('400')
  await expect(dialog.getByTestId('return-total')).toHaveText(money('Итого: 400 с'))
  await dialog.getByRole('button', { name: 'Провести возврат' }).click()
  await expect(dialog).toBeHidden()
  await expect(page.getByRole('tab', { name: 'Возвраты' })).toHaveAttribute('aria-selected', 'true')
  await expect(page.getByRole('row', { name: new RegExp(`Лампа ${RUN}`) })).toContainText('в счёт долга')
  const party = await apiGet<{ balance_tyiyn: number }>(page.request, `/parties/${supplier.id}`)
  expect(party.balance_tyiyn).toBe(-60_000)
  const stock = await apiGet<{ stock_qty: number }>(page.request, `/products/${p.id}`)
  expect(stock.stock_qty).toBe(6)
  await page.getByRole('tab', { name: 'Поставки' }).click()
  await expect(page.getByRole('columnheader', { name: 'Вернули' })).toBeVisible()
  await expect(page.getByRole('row', { name: new RegExp(`Лампа ${RUN}`) })).toContainText('4 шт')
  // Платёж в истории — «возврат товара поставщику».
  await page.getByRole('tab', { name: 'Платежи' }).click()
  await expect(page.getByText('возврат товара поставщику').first()).toBeVisible()
})

test('ревизия и товары — один экран; в ревизию вписывают товар, которого нет в программе', async ({ page }) => {
  await login(page)
  const catName = `Антифризы ${RUN}`
  await apiPost(page.request, '/categories', { name: catName, kind: 'other' })
  await page.goto('/intake')
  await expect(page).toHaveURL(/\/revision\?tab=scan$/)
  await expect(page.getByRole('tab', { name: 'Сканер товаров' })).toHaveAttribute('aria-selected', 'true')
  await page.getByRole('tab', { name: 'Ревизия' }).click()
  await page.getByRole('button', { name: 'Начать ревизию' }).click()
  await page.getByRole('button', { name: '+ Товар не из программы' }).click()
  const dialog = page.getByRole('dialog', { name: 'Товар, которого нет в программе' })
  const name = `Антифриз G12 ${RUN}`
  await dialog.getByPlaceholder('Название, артикул или штрихкод').fill(name)
  await dialog.getByRole('button', { name: `+ Завести товар «${name}»` }).click()
  await dialog.locator('#revnew-category').selectOption({ label: catName })
  await dialog.locator('#revnew-price').fill('600')
  await dialog.locator('#revnew-cost').fill('450')
  await dialog.locator('#revnew-count').fill('5')
  await dialog.getByRole('button', { name: 'Завести и посчитать' }).click()
  await expect(dialog).toBeHidden()
  const row = page.getByRole('row', { name: new RegExp(name) })
  await expect(row).toBeVisible()
  await expect(row.locator('input')).toHaveValue('5')
  // Проводит владелец: остаток 5 шт по закупке 450 с.
  await page.getByRole('button', { name: 'Провести' }).click()
  await page.getByLabel('Кто пересчитывал, причина').fill('проверка')
  await page.getByRole('dialog').getByRole('button', { name: 'Провести' }).click()
  await expect(page.getByText('Ревизия проведена')).toBeVisible()
  const found = await apiGet<{ id: string; stock_qty: number }[]>(page.request, `/products?q=${encodeURIComponent(name)}`)
  expect(found[0].stock_qty).toBe(5)
})
