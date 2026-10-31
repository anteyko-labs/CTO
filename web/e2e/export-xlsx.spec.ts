// «В Excel» у таблиц (задача 003 Н13, ADR-159): в файле все строки отбора, а не страница; деньги и
// количества — числами, коды — текстом с нулями; отчёт смены — листами одного файла; акт сверки —
// «Скачать». Администратору — только то, что ему видно: без закупки и стоимости остатка (I13).
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ownerSafe } from './safe'
import { readBook } from './xlsx'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e313' }
const RUN = Date.now().toString().slice(-7)
const two = (i: number) => String(i).padStart(2, '0')

async function login(page: Page, user = OWNER, password = OWNER_PASSWORD) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(user)
  await page.getByLabel('Пароль').fill(password)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' }).first()).toBeVisible()
}

async function apiPost<T>(request: APIRequestContext, path: string, data: unknown): Promise<T> {
  const res = await request.post(`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

/** Нажать кнопку выгрузки и прочитать файл: имена листов и строки текстом. */
async function download(page: Page, name: string, click: () => Promise<void>) {
  const wait = page.waitForEvent('download')
  await click()
  const file = await wait
  expect(file.suggestedFilename()).toMatch(/\.xlsx$/)
  const path = test.info().outputPath(`${name}.xlsx`)
  await file.saveAs(path)
  return { ...readBook(path), filename: file.suggestedFilename() }
}

/** Строка шапки таблицы листа: первая строка, начинающаяся с `first`. */
const header = (rows: string[][], first: string) => rows.find((r) => r[0] === first) ?? []

test('выгрузка: приход больше страницы, товары, остатки, долги, акт сверки; администратору — без закупки', async ({ page, browser }) => {
  await login(page)
  const request = page.request
  const safe = await ownerSafe(request)
  const category = `Выгрузка ${RUN}`
  const cat = await apiPost<{ id: string }>(request, '/categories', { name: category, kind: 'other' })
  const productName = `Фильтр выгрузки ${RUN}`
  const barcode = `0${RUN}55`
  const product = await apiPost<{ id: string }>(request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: productName,
    article: '',
    sale_price_tyiyn: 20_050,
    min_stock: 0,
    barcodes: [barcode],
  })
  // 30 накладных одного поставщика по 1 шт × 100 с: при странице 25 в файле — все 30.
  const supplierName = `Поставщик выгрузки ${RUN}`
  const supplier = await apiPost<{ id: string }>(request, '/suppliers', { name: supplierName, phone: '', comment: '' })
  for (let i = 1; i <= 30; i++) {
    await apiPost(request, '/receipts', {
      op_id: crypto.randomUUID(),
      supplier_id: supplier.id,
      payment: 'paid',
      paid_from: safe,
      supplier_doc: `ВЫГ-${RUN}-${two(i)}`,
      lines: [{ product_id: product.id, qty: 1, cost_tyiyn: 10_000 }],
    })
  }

  // «Приход»: поиск по поставщику, страница 25 — в файле все 30 строк, суммы числами.
  await page.goto('/receipts')
  await page.locator('#receipts-search').fill(supplierName)
  const pager = page.getByRole('navigation', { name: 'Страницы приходов' })
  await expect(pager.getByTestId('pager-range')).toHaveText(/\/ 30$/)
  await pager.getByLabel('Строк на странице').selectOption('25')
  await expect(pager.getByTestId('pager-range')).toHaveText('1–25 / 30')
  const receipts = await download(page, 'receipts', () => page.getByTestId('export-xlsx').click())
  expect(receipts.filename).toMatch(/^Приход/)
  const rHead = header(receipts.sheets[0], '№')
  expect(rHead.slice(0, 7)).toEqual(['№', 'Дата', 'Вид', 'Поставщик / откуда', 'Документ поставщика', 'Комментарий', 'Сумма'])
  const rows = receipts.sheets[0].filter((r) => r[3] === supplierName)
  expect(rows).toHaveLength(30)
  expect(rows.every((r) => r[6] === '100.00')).toBeTruthy()
  expect(rows.map((r) => r[4]).sort()[0]).toBe(`ВЫГ-${RUN}-01`)
  expect(receipts.sheets[0].some((r) => r[0].startsWith('Отбор: поиск'))).toBeTruthy()

  // «Товары» у владельца: закупка и сумма по закупке есть; штрихкод — текстом с нулём впереди.
  await page.goto('/products')
  await page.getByLabel('Категория').selectOption({ label: category })
  await expect(page.getByRole('table', { name: 'Список товаров' }).getByRole('row', { name: new RegExp(productName) })).toBeVisible()
  const products = await download(page, 'products-owner', () => page.getByTestId('export-xlsx').click())
  const pHead = header(products.sheets[0], 'Название')
  expect(pHead).toContain('Закупка')
  expect(pHead).toContain('Сумма по закупке')
  const pRow = products.sheets[0].find((r) => r[0] === productName) ?? []
  expect(pRow[pHead.indexOf('Штрихкоды')]).toBe(barcode)
  expect(pRow[pHead.indexOf('Остаток')]).toBe('30')
  expect(pRow[pHead.indexOf('Продажа')]).toBe('200.50')
  expect(pRow[pHead.indexOf('Сумма по закупке')]).toBe('3000.00')

  // «Остатки» у владельца: «Сумма (закупка)» есть, категория у строки, подытог категории.
  await page.goto('/stock')
  await page.getByTestId('stock-search').fill(productName)
  await expect(page.getByTestId('stock-row')).toHaveCount(1)
  const stock = await download(page, 'stock-owner', () => page.getByTestId('export-xlsx').click())
  const sHead = header(stock.sheets[0], 'Категория')
  expect(sHead).toContain('Сумма (закупка)')
  const sRow = stock.sheets[0].find((r) => r[1] === productName) ?? []
  expect(sRow[0]).toBe(category)
  expect(sRow[sHead.indexOf('Остаток')]).toBe('30')
  expect(sRow[sHead.indexOf('Сумма (продажа)')]).toBe('6015.00')
  expect(stock.sheets[0].some((r) => r[1] === 'Итого по категории: 1 поз.')).toBeTruthy()

  // «Долги»: старый долг клиента — в листе «Должны нам», итог листа = сумма столбца «Долг».
  const clientName = `Должник выгрузки ${RUN}`
  const party = await apiPost<{ id: string }>(request, '/parties', { role: 'customer', kind: 'person', name: clientName })
  const debtDate = new Date(Date.now() - 5 * 86_400_000).toISOString().slice(0, 10)
  await apiPost(request, '/debts/old', { op_id: crypto.randomUUID(), party_id: party.id, amount_tyiyn: 123_450, date: debtDate, comment: 'тетрадь' })
  await page.goto('/debts')
  await expect(page.getByRole('row', { name: new RegExp(clientName) })).toBeVisible()
  const debts = await download(page, 'debts', () => page.getByTestId('export-xlsx').click())
  expect(debts.names).toEqual(['Должны нам', 'Должны мы'])
  const dHead = header(debts.sheets[0], 'Контрагент')
  const debtCol = dHead.indexOf('Долг')
  const dRow = debts.sheets[0].find((r) => r[0] === clientName) ?? []
  expect(dRow[debtCol]).toBe('1234.50')
  expect(dRow[dHead.indexOf('Вид')]).toBe('Физлицо')
  const body = debts.sheets[0].slice(debts.sheets[0].indexOf(dHead) + 1).filter((r) => r[0] !== 'Итого')
  const totalRow = debts.sheets[0].find((r) => r[0] === 'Итого') ?? []
  const cents = (s: string) => Math.round(Number(s) * 100)
  expect(cents(totalRow[debtCol])).toBe(body.reduce((a, r) => a + (r[debtCol] ? cents(r[debtCol]) : 0), 0))

  // Акт сверки клиента: «Скачать» — .xlsx со строкой долга и сальдо числами.
  await page.goto(`/clients/${party.id}`)
  await page.getByRole('button', { name: 'Акт сверки' }).click()
  const act = page.getByTestId('reconciliation')
  await expect(act.getByTestId('act-closing')).toBeVisible()
  const actBook = await download(page, 'act', () => act.getByTestId('act-download').click())
  expect(actBook.filename).toMatch(/^Акт сверки /)
  expect(actBook.names).toEqual(['Акт сверки'])
  const aHead = header(actBook.sheets[0], 'Дата')
  expect(aHead).toEqual(['Дата', 'Документ', 'Комментарий', 'Дебет', 'Кредит'])
  const saldo = actBook.sheets[0].filter((r) => r[1]?.startsWith('Сальдо на'))
  expect(saldo).toHaveLength(2)
  expect(saldo[1][3]).toBe('1234.50')
  // Строка долга — с датой (числом Excel) и суммой в дебете.
  expect(actBook.sheets[0].some((r) => /^\d+$/.test(r[0]) && r[3] === '1234.50')).toBeTruthy()

  // Администратор: в «Товарах» и «Остатках» нет закупки и стоимости по закупке.
  const admin = `xls${RUN}`
  const password = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  await apiPost(request, '/users', { login: admin, password, full_name: `Админ ${RUN}`, role: 'admin' })
  const ctx = await browser.newContext()
  const a = await ctx.newPage()
  await login(a, admin, password)
  await a.goto('/products')
  await a.getByLabel('Категория').selectOption({ label: category })
  await expect(a.getByRole('table', { name: 'Список товаров' }).getByRole('row', { name: new RegExp(productName) })).toBeVisible()
  const aProducts = await download(a, 'products-admin', () => a.getByTestId('export-xlsx').click())
  const apHead = header(aProducts.sheets[0], 'Название')
  expect(apHead).not.toContain('Закупка')
  expect(apHead).not.toContain('Сумма по закупке')
  expect(aProducts.sheets[0].find((r) => r[0] === productName)).toBeTruthy()
  expect(aProducts.sheets[0].flat()).not.toContain('3000.00')
  await a.goto('/stock')
  await a.getByTestId('stock-search').fill(productName)
  await expect(a.getByTestId('stock-row')).toHaveCount(1)
  const aStock = await download(a, 'stock-admin', () => a.getByTestId('export-xlsx').click())
  expect(header(aStock.sheets[0], 'Категория')).not.toContain('Сумма (закупка)')
  expect(aStock.sheets[0].flat()).not.toContain('3000.00')
  await ctx.close()
})

test('отчёт смены — одним файлом, вкладки листами; чек в «Продажах» числом', async ({ page }) => {
  await login(page)
  const request = page.request
  const cashier = await apiPost<{ id: string }>(request, '/employees', { full_name: `Кассир выгрузки ${RUN}`, is_cashier: true, pin: '2468' })
  const cat = await apiPost<{ id: string }>(request, '/categories', { name: `Смена выгрузки ${RUN}`, kind: 'other' })
  const p = await apiPost<{ id: string }>(request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Свеча выгрузки ${RUN}`,
    article: '',
    sale_price_tyiyn: 45_000,
    min_stock: 0,
  })
  await apiPost(request, '/receipts', { op_id: crypto.randomUUID(), paid_from: await ownerSafe(request), lines: [{ product_id: p.id, qty: 2, cost_tyiyn: 60_000 }] })
  const current = await request.get('/api/v1/shifts/current')
  let shift = (await current.json()) as { id: string } | null
  if (!shift) shift = await apiPost<{ id: string }>(request, '/shifts/open', { op_id: crypto.randomUUID(), cashier_employee_id: cashier.id, cashier_pin: '2468' })
  const sale = await apiPost<{ id: string; number: number }>(request, '/sales', {
    op_id: crypto.randomUUID(),
    client_time: new Date().toISOString(),
    sale_type: 'takeaway',
    cashier_id: cashier.id,
    master_id: null,
    party_id: null,
    contact_id: null,
    vehicle_id: null,
    comment: '',
    lines: [{ kind: 'piece', gift: false, product_id: p.id, service_id: null, qty: 1, unit_price_tyiyn: 45_000 }],
    payments: [{ method: 'cash', amount_tyiyn: 45_000 }],
  })

  await page.goto(`/shifts/${shift.id}`)
  await expect(page.getByTestId('shift-report')).toBeVisible()
  const book = await download(page, 'shift', () => page.getByTestId('export-xlsx').click())
  expect(book.filename).toMatch(/^Смена \d+ /)
  expect(book.names).toEqual(['Обзор', 'Продажи', 'Возвраты', 'Приходы', 'Деньги', 'Долги', 'Выплаты', 'Отменённое'])
  expect(book.sheets[0].some((r) => r[0] === 'Выручка')).toBeTruthy()
  const sales = book.sheets[1]
  const head = header(sales, '№')
  expect(head).toEqual(['№', 'Время', 'Способ', 'Счёт', 'Кассир', 'Покупатель', 'Сумма'])
  const row = sales.find((r) => r[0] === String(sale.number))
  expect(row?.[6]).toBe('450.00')
})
