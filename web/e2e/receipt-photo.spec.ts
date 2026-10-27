// Накладная по фото (SPEC-30 §9): черновик открывает заполненный «Новый приход» — поставщик по
// названию, товары по штрихкоду, артикулу и похожему названию, литры масла в канистрах, строка
// без товара; итог сверяется; после проведения фото — у накладной. Сервис распознавания —
// подставной (здесь же, на E2E_ASSISTANT_PORT): сервер запускают с ASSISTANT_API_KEY и
// ASSISTANT_API_URL=http://127.0.0.1:<порт>. Без них распознавание выключено — проверка пропускается.
import http from 'node:http'
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2f9' }
const RUN = Date.now().toString().slice(-8)
const PORT = Number(process.env.E2E_ASSISTANT_PORT ?? '8096')
const money = (text: string) => new RegExp(text.replace(/ /g, '\\s'))
/** Снимок 1×1 точка — «фото накладной». */
const PNG = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==', 'base64')

/** Накладная, которую «прочитал» подставной сервис. */
const INVOICE = {
  readable: true,
  supplier_name: `ОсОО «Фотопоставщик ${RUN}»`,
  supplier_inn: '',
  doc_number: '15',
  doc_date: '2026-10-20',
  lines: [
    { name: `Фильтр воздушный ${RUN}`, code: '', barcode: `46${RUN}501`, quantity: '2', unit: 'шт', price: '350', sum: '700' },
    { name: 'Масло моторное 5W-30 разлив', code: `OIL-${RUN}`, barcode: '', quantity: '8', unit: 'л', price: '450', sum: '3600' },
    { name: `Колодки тормозные передние Brembo ${RUN}`, code: '', barcode: '', quantity: '1', unit: 'шт', price: '1500', sum: '1500' },
    { name: 'Щёткодержатель зеркала XQZ', code: '', barcode: '', quantity: '3', unit: 'шт', price: '100', sum: '300' },
  ],
  total: '6100',
}

/** Что «прочитает» сервис в текущей проверке. */
let invoice: typeof INVOICE = INVOICE

let service: http.Server | null = null

test.beforeAll(async () => {
  service = http.createServer((req, res) => {
    req.resume()
    req.on('end', () => {
      res.writeHead(200, { 'Content-Type': 'application/json' })
      res.end(
        JSON.stringify({
          candidates: [{ content: { role: 'model', parts: [{ text: JSON.stringify(invoice) }] }, finishReason: 'STOP' }],
          usageMetadata: { promptTokenCount: 1000, candidatesTokenCount: 200 },
        }),
      )
    })
  })
  await new Promise<void>((resolve) => service!.listen(PORT, '127.0.0.1', resolve))
})

test.afterAll(async () => {
  await new Promise<void>((resolve) => (service ? service.close(() => resolve()) : resolve()))
})

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

test('накладная по фото открывает заполненный приход, фото — у накладной', async ({ page }) => {
  await login(page)
  const enabled = (await apiGet<{ enabled: boolean }>(page.request, '/receipt-drafts')).enabled
  test.skip(!enabled, 'распознавание накладных не настроено на сервере (ASSISTANT_API_KEY, ASSISTANT_API_URL)')

  const other = await apiPost<{ id: string }>(page.request, '/categories', { name: `Фото ${RUN}`, kind: 'other' })
  const oils = await apiPost<{ id: string }>(page.request, '/categories', { name: `Масла фото ${RUN}`, kind: 'oil' })
  const supplier = await apiPost<{ id: string }>(page.request, '/suppliers', { name: `Фотопоставщик ${RUN}` })
  const filter = await apiPost<{ id: string }>(page.request, '/products', {
    op_id: crypto.randomUUID(), category_id: other.id, name: `Фильтр воздушный ${RUN}`, barcodes: [`46${RUN}501`], sale_price_tyiyn: 50_000,
  })
  const oil = await apiPost<{ id: string }>(page.request, '/products', {
    op_id: crypto.randomUUID(), category_id: oils.id, name: `Масло фото ${RUN} 4л`, article: `OIL ${RUN}`, container_ml: 4000, sale_price_tyiyn: 250_000,
  })
  await apiPost(page.request, '/products', {
    op_id: crypto.randomUUID(), category_id: other.id, name: `Колодки Brembo ${RUN}`, sale_price_tyiyn: 200_000,
  })
  await apiPost(page.request, '/products', {
    op_id: crypto.randomUUID(), category_id: other.id, name: `Держатель ${RUN}`, sale_price_tyiyn: 20_000,
  })

  // Фото пришло (в боте или с экрана) — черновик в «Приходе» → «Накладные по фото».
  const res = await page.request.post(`/api/v1/receipt-drafts?op_id=${crypto.randomUUID()}&name=nakladnaya.png`, {
    data: PNG,
    headers: { ...DEVICE, 'Content-Type': 'image/png' },
  })
  expect(res.ok(), await res.text()).toBeTruthy()
  const draft = (await res.json()) as { id: string }

  await page.goto('/receipts')
  const row = page.getByTestId('receipt-draft').filter({ hasText: `Фотопоставщик ${RUN}` })
  await expect(row).toContainText('4 строки')
  await expect(row).toContainText(money('итого 6 100 с'))
  await row.getByRole('button', { name: 'Открыть' }).click()
  await expect(page).toHaveURL(new RegExp(`/receipts/new\\?draft=${draft.id}`))

  // Заполнено: поставщик по названию, номер и дата, три строки с товаром и одна без.
  await expect(page.getByTestId('draft-banner')).toContainText('Поставщик найден по названию')
  await expect(page.locator('#receipt-supplier')).toHaveValue(supplier.id)
  await expect(page.getByLabel('Номер документа поставщика')).toHaveValue('15 от 20.10.2026')
  await expect(page.locator('#line-1-count')).toHaveValue('2')
  await expect(page.locator('#line-1-price')).toHaveValue('350')
  // 8 л по 450 с → 2 канистры по 4 л по 1 800 с.
  await expect(page.locator('#line-2-count')).toHaveValue('2')
  await expect(page.locator('#line-2-price')).toHaveValue('1800')
  await expect(page.getByTestId('draft-hint').nth(1)).toContainText(money('8 л = 2 кан. по 4 л'))
  await expect(page.getByTestId('draft-hint').nth(2)).toContainText('проверьте')
  await expect(page.locator('#line-3-count')).toHaveValue('1')
  const unmatched = page.getByTestId('draft-unmatched-line')
  await expect(unmatched).toHaveCount(1)
  await expect(unmatched).toContainText('не найден — выберите товар или заведите новый')
  // Итог накладной 6 100 с, строк с товаром — на 5 800 с: расхождение подсвечено, провести нельзя.
  await expect(page.getByTestId('draft-total-mismatch')).toHaveText(money('Итог накладной 6 100 с, сумма строк 5 800 с — расходится на 300 с'))
  await expect(page.getByText('строки накладной без товара — выберите товар или уберите строку')).toBeVisible()

  // Строке без товара выбирают товар в поиске: количество и цена — из накладной.
  await unmatched.getByRole('button', { name: 'Выбрать товар' }).click()
  await expect(page.getByTestId('draft-target')).toBeVisible()
  await page.locator('#receipt-picker').fill(`Держатель ${RUN}`)
  await page.getByText(`Держатель ${RUN}`).first().click()
  await expect(page.getByTestId('draft-unmatched')).toBeHidden()
  await expect(page.locator('#line-4-count')).toHaveValue('3')
  await expect(page.locator('#line-4-price')).toHaveValue('100')
  await expect(page.getByTestId('draft-total-ok')).toBeVisible()
  await expect(page.getByText(money('Итого: 6 100 с'))).toBeVisible()

  // Проводит человек: в долг поставщику. Фото прикрепляется к накладной.
  await page.getByLabel('Вся в долг').check()
  const print = page.getByLabel('Печатать этикетки')
  if (await print.isChecked()) await print.uncheck()
  await page.getByRole('button', { name: 'Провести приход' }).first().click()
  await expect(page).toHaveURL(/\/receipts\/[0-9a-f-]{36}$/)
  await expect(page.getByTestId('receipt-photos').locator('img')).toHaveCount(1)
  const id = page.url().split('/').pop()!
  const files = await apiGet<{ name: string }[]>(page.request, `/receipts/${id}/files`)
  expect(files.map((f) => f.name)).toEqual(['nakladnaya.png'])
  const used = await apiGet<{ status: string; receipt_id: string }>(page.request, `/receipt-drafts/${draft.id}`)
  expect(used).toMatchObject({ status: 'used', receipt_id: id })
  const receipt = await apiGet<{ lines: { product_id: string; qty: number; cost_tyiyn: number }[] }>(page.request, `/receipts/${id}`)
  expect(receipt.lines.find((l) => l.product_id === oil.id)).toMatchObject({ qty: 8000, cost_tyiyn: 360_000 })
  expect(receipt.lines.find((l) => l.product_id === filter.id)).toMatchObject({ qty: 2, cost_tyiyn: 70_000 })

  // «Заполнить по фото» в «Новом приходе»: файл — тот же черновик на экране; «Убрать» в «Приходе».
  await page.goto('/receipts/new')
  await page.getByTestId('fill-photo-input').setInputFiles({ name: 'second.png', mimeType: 'image/png', buffer: PNG })
  await expect(page.getByTestId('draft-banner')).toBeVisible()
  await expect(page.locator('#line-1-count')).toHaveValue('2')
  await page.goto('/receipts')
  const again = page.getByTestId('receipt-draft').filter({ hasText: `Фотопоставщик ${RUN}` })
  await expect(again).toHaveCount(1)
  page.once('dialog', (d) => void d.accept())
  await again.getByRole('button', { name: 'Убрать' }).click()
  await expect(again).toHaveCount(0)
})

test('накладная по фото: 18 л масла не делятся на канистры — строка в литрах с ценой литра', async ({ page }) => {
  await login(page)
  const enabled = (await apiGet<{ enabled: boolean }>(page.request, '/receipt-drafts')).enabled
  test.skip(!enabled, 'распознавание накладных не настроено на сервере (ASSISTANT_API_KEY, ASSISTANT_API_URL)')
  const oils = await apiPost<{ id: string }>(page.request, '/categories', { name: `Масла литрами фото ${RUN}`, kind: 'oil' })
  const oil = await apiPost<{ id: string }>(page.request, '/products', {
    op_id: crypto.randomUUID(), category_id: oils.id, name: `Масло бочка фото ${RUN}`, article: `BOIL-${RUN}`, container_ml: 4000, sale_price_tyiyn: 250_000,
  })
  invoice = {
    ...INVOICE,
    supplier_name: `Литровый поставщик ${RUN}`,
    lines: [{ name: 'Масло 5W-30 разлив', code: `BOIL-${RUN}`, barcode: '', quantity: '18', unit: 'л', price: '450', sum: '8100' }],
    total: '8100',
  }
  try {
    const res = await page.request.post(`/api/v1/receipt-drafts?op_id=${crypto.randomUUID()}&name=litry.png`, {
      data: PNG,
      headers: { ...DEVICE, 'Content-Type': 'image/png' },
    })
    expect(res.ok(), await res.text()).toBeTruthy()
    const draft = (await res.json()) as { id: string }
    await page.goto(`/receipts/new?draft=${draft.id}`)
    await expect(page.getByTestId('draft-banner')).toBeVisible()
    await expect(page.getByTestId('line-1-mode-l')).toHaveAttribute('aria-checked', 'true')
    await expect(page.locator('#line-1-count')).toHaveValue('18')
    await expect(page.locator('#line-1-price')).toHaveValue('450')
    await expect(page.getByTestId('draft-hint')).toContainText('строка в литрах, цена за литр')
    await expect(page.getByTestId('receipt-total')).toHaveText(money('8 100 с'))
    await expect(page.getByTestId('draft-total-ok')).toBeVisible()
    expect(oil.id).toBeTruthy()
    await page.goto('/receipts')
    const row = page.getByTestId('receipt-draft').filter({ hasText: `Литровый поставщик ${RUN}` })
    page.once('dialog', (d) => void d.accept())
    await row.getByRole('button', { name: 'Убрать' }).click()
    await expect(row).toHaveCount(0)
  } finally {
    invoice = INVOICE
  }
})
