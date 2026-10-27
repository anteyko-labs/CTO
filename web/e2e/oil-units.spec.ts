// Канистры и литры, вязкость, вес (SPEC-36 §13, ADR-123): ревизия масла «кан. + л», скан канистры —
// +1 канистра, «Должно быть» в виде «8 кан. + 2,5 л», вязкость бейджем, поиском и отбором; приход
// масла литрами по цене литра и его «Исправить»; весовой товар — в кг в ревизии и в приходе.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e36d' }
const RUN = Date.now().toString().slice(-8)
/** Текст с неразрывными пробелами: «8 кан. + 2,5 л» — как на экране. */
const money = (text: string) => new RegExp(text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&').replace(/ /g, '\\s'))

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

/** Масло 4 л с остатком `stockMl` (приходом в долг по 1 800 с за канистру). */
async function oil(request: APIRequestContext, cat: string, name: string, code: string, stockMl: number, attrs: Record<string, string> = {}) {
  const p = await apiPost<{ id: string }>(request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat,
    name,
    container_ml: 4000,
    barcodes: [code],
    sale_price_tyiyn: 250_000,
    attrs,
  })
  if (stockMl > 0) {
    const supplier = await apiPost<{ id: string }>(request, '/suppliers', { name: `Поставщик масла ${name}` })
    await apiPost(request, '/receipts', {
      op_id: crypto.randomUUID(),
      supplier_id: supplier.id,
      payment: 'debt',
      lines: [{ product_id: p.id, qty: stockMl, cost_tyiyn: (stockMl / 4000) * 180_000 }],
    })
  }
  return p
}

const startRevision = async (page: Page, category: string) => {
  await page.goto('/revision')
  await page.getByLabel('Что пересчитываем').selectOption({ label: category })
  await page.getByRole('button', { name: 'Начать ревизию' }).click()
  await expect(page.getByRole('heading', { name: /Ревизия № \d+/ })).toBeVisible()
}

const cancelRevision = async (page: Page) => {
  await page.getByRole('button', { name: 'Отменить ревизию' }).click()
  await expect(page.getByRole('button', { name: 'Отменить ревизию' })).toHaveCount(0)
}

test('ревизия масла: «кан. + л», скан канистры, «Должно быть» канистрами, вязкость', async ({ page }) => {
  await login(page)
  const catName = `Масла ревизии ${RUN}`
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: catName, kind: 'oil' })
  const codeA = `31${RUN}01`
  const a = await oil(page.request, cat.id, `Масло Кан ${RUN} 5W30 4L`, codeA, 34_500)
  const b = await oil(page.request, cat.id, `Масло Бэ ${RUN} 4 л`, `31${RUN}02`, 8000, { viscosity: '10W-40' })

  await startRevision(page, catName)
  const rowA = page.getByRole('row', { name: new RegExp(`Масло Кан ${RUN}`) })
  const rowB = page.getByRole('row', { name: new RegExp(`Масло Бэ ${RUN}`) })
  // «Должно быть» — канистрами и литрами.
  await expect(rowA.getByTestId('expected')).toHaveText(money('8 кан. + 2,5 л'))
  await expect(rowB.getByTestId('expected')).toHaveText(money('2 кан.'))
  // Вязкость: из карточки — бейдж, без неё — серым из названия.
  await expect(rowB.getByTestId('viscosity')).toHaveText('10W-40')
  await expect(rowA.getByTestId('viscosity-from-name')).toContainText('5W-30 · из названия — заполните в карточке')
  // Отбор по вязкости — по строкам ревизии.
  await page.locator('#revision-viscosity').selectOption({ label: '10W-40' })
  await expect(rowB).toBeVisible()
  await expect(rowA).toHaveCount(0)
  await page.locator('#revision-viscosity').selectOption({ label: 'все' })
  // Поиск по вязкости — «5w30» находит «5W-30» из названия.
  await page.getByPlaceholder('Название, артикул или вязкость').fill('5w30')
  await expect(rowA).toBeVisible()
  await expect(rowB).toHaveCount(0)
  await page.getByPlaceholder('Название, артикул или вязкость').fill('')

  // Скан канистры — +1 канистра, курсор остаётся в сканере.
  const scanner = page.getByPlaceholder('Сканируйте штрихкод')
  await scanner.fill(codeA)
  await scanner.press('Enter')
  const cans = page.locator(`[data-count="${a.id}"]`)
  const liters = page.locator(`[data-count-l="${a.id}"]`)
  await expect(cans).toHaveValue('1')
  await expect(rowA.getByTestId('oil-count-total')).toHaveText(money('= 4 л'))
  await expect(scanner).toBeFocused()
  await scanner.fill(codeA)
  await scanner.press('Enter')
  await expect(cans).toHaveValue('2')

  // Початая тара — литрами: «2 кан. + 1,5 л» = 9 500 мл.
  await liters.fill('1,5')
  await expect(rowA.getByTestId('oil-count-total')).toHaveText(money('= 9,5 л'))
  await scanner.focus()
  await expect(rowA).toContainText(money('−6 кан. + 1 л'))
  const list = await apiGet<{ id: string; status: string; category_id: string | null }[]>(page.request, '/revisions')
  const mine = list.find((r) => r.status === 'draft' && r.category_id === cat.id)
  expect(mine).toBeTruthy()
  const out = await apiGet<{ lines: { product_id: string; counted_qty: number | null }[] }>(page.request, `/revisions/${mine!.id}`)
  expect(out.lines.find((l) => l.product_id === a.id)?.counted_qty).toBe(9500)
  expect(b.id).toBeTruthy()
  await cancelRevision(page)
})

test('вес в ревизии и в приходе — в кг: правка показанного числа не умножает на 1000', async ({ page }) => {
  await login(page)
  const info = await apiGet<{ product_id: string; sale_per_kg_tyiyn: number }>(page.request, '/batteries')
  const scrap = await apiGet<{ id: string; name: string; category_id: string; barcodes: string[] }>(page.request, `/products/${info.product_id}`)
  const cats = await apiGet<{ id: string; name: string }[]>(page.request, '/categories')
  const catName = cats.find((c) => c.id === scrap.category_id)!.name

  await startRevision(page, catName)
  const field = page.locator(`[data-count="${scrap.id}"]`)
  await expect(field).toHaveAttribute('placeholder', 'кг')
  const list = await apiGet<{ id: string; status: string; category_id: string | null }[]>(page.request, '/revisions')
  const mine = list.find((r) => r.status === 'draft' && r.category_id === scrap.category_id)!
  const counted = async () => (await apiGet<{ counted_qty: number | null }>(page.request, `/revisions/${mine.id}/lines/${scrap.id}`)).counted_qty
  await field.fill('12,5')
  await field.press('Enter')
  await expect.poll(counted).toBe(12_500)
  await expect(field).toHaveValue('12,5')
  // Поправили показанное число — 12,6 кг, а не 12 600 кг.
  await field.fill('12,6')
  await field.press('Enter')
  await expect.poll(counted).toBe(12_600)
  await expect(field).toHaveValue('12,6')
  await cancelRevision(page)

  // Приход весового товара: 12,5 кг по 100 с за кг = 1 250 с; по продаже — цена за кг × 12,5.
  await page.goto('/receipts/new')
  await page.locator('#receipt-picker').fill(scrap.barcodes[0])
  await page.locator('#receipt-picker').press('Enter')
  await expect(page.getByLabel('Кг', { exact: true })).toHaveValue('1')
  await page.getByLabel('Кг', { exact: true }).fill('12,5')
  await page.locator('#line-1-price').fill('100')
  await expect(page.getByText('Цена за кг, с')).toBeVisible()
  await expect(page.getByTestId('receipt-total')).toHaveText(money('1 250 с'))
  const sale = (info.sale_per_kg_tyiyn * 125) / 10
  const som = Math.trunc(sale / 100)
  const tyiyn = Math.round(sale) % 100
  const saleText = `${som.toLocaleString('ru-RU').replace(/ /g, ' ')}${tyiyn ? `,${String(tyiyn).padStart(2, '0')}` : ''} с`
  await expect(page.getByTestId('receipt-sale-total')).toHaveText(money(saleText))
})

test('ревизия: две правки строки подряд — сохраняется последняя, даже если первая дошла до сервера позже', async ({ page }) => {
  await login(page)
  const info = await apiGet<{ product_id: string }>(page.request, '/batteries')
  const scrap = await apiGet<{ id: string; category_id: string }>(page.request, `/products/${info.product_id}`)
  const cats = await apiGet<{ id: string; name: string }[]>(page.request, '/categories')
  await startRevision(page, cats.find((c) => c.id === scrap.category_id)!.name)
  const list = await apiGet<{ id: string; status: string; category_id: string | null }[]>(page.request, '/revisions')
  const mine = list.find((r) => r.status === 'draft' && r.category_id === scrap.category_id)!
  const counted = async () => (await apiGet<{ counted_qty: number | null }>(page.request, `/revisions/${mine.id}/lines/${scrap.id}`)).counted_qty
  // Первая запись идёт медленно (загруженный сервер): вторая не должна её обогнать.
  let first = true
  await page.route(/\/api\/v1\/revisions\/[0-9a-f-]+\/lines$/, async (route) => {
    if (first) {
      first = false
      await new Promise((r) => setTimeout(r, 1_500))
    }
    await route.continue()
  })
  const field = page.locator(`[data-count="${scrap.id}"]`)
  await field.fill('12,5')
  await field.press('Enter')
  await field.fill('12,6')
  await field.press('Enter')
  await page.waitForTimeout(2_500)
  await expect.poll(counted).toBe(12_600)
  await expect(field).toHaveValue('12,6')
  await page.unroute(/\/api\/v1\/revisions\/[0-9a-f-]+\/lines$/)
  await cancelRevision(page)
})

test('приход масла литрами: 18 л по 450 с = 8 100 с, остаток +18 л; «Исправить» — в литрах', async ({ page }) => {
  await login(page)
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Масла прихода ${RUN}`, kind: 'oil' })
  const code = `32${RUN}01`
  const p = await oil(page.request, cat.id, `Масло литрами ${RUN}`, code, 4000)
  const supplier = await apiPost<{ id: string }>(page.request, '/suppliers', { name: `Поставщик литров ${RUN}` })

  await page.goto('/receipts/new')
  await page.locator('#receipt-picker').fill(code)
  await page.locator('#receipt-picker').press('Enter')
  await expect(page.locator('#line-1-count')).toHaveValue('1')
  // В приходе по умолчанию — канистры; прошлая закупка — за канистру.
  await expect(page.getByTestId('line-1-mode-can')).toHaveAttribute('aria-checked', 'true')
  await expect(page.getByTestId('last-receipt')).toContainText(money('в прошлый раз: 1 800 с за канистру'))
  await expect(page.locator('#line-1-price')).toHaveValue('1800')
  await page.getByTestId('line-1-mode-l').click()
  await expect(page.locator('#line-1-count')).toHaveValue('4')
  await expect(page.locator('#line-1-price')).toHaveValue('450')
  await expect(page.getByText('Цена за литр, с')).toBeVisible()
  await page.locator('#line-1-count').fill('18')
  await page.locator('#line-1-price').fill('450')
  await expect(page.getByTestId('liters-split')).toHaveText(money('= 4 кан. + 2 л'))
  await expect(page.getByTestId('liters-per-can')).toHaveText(money('за канистру 4 л ≈ 1 800 с'))
  await expect(page.getByTestId('receipt-total')).toHaveText(money('8 100 с'))
  // Цена продажи и наценка — за канистру: 1 800 → 2 500 = 38,9 %.
  await expect(page.getByLabel('Наценка, %')).toHaveValue('38,9')
  await expect(page.getByTestId('receipt-sale-total')).toHaveText(money('11 250 с'))
  await page.locator('#receipt-supplier').selectOption(supplier.id)
  await page.getByLabel('Вся в долг').check()
  const print = page.getByLabel('Печатать этикетки')
  if (await print.isChecked()) await print.uncheck()
  await page.getByRole('button', { name: 'Провести приход' }).first().click()
  await expect(page).toHaveURL(/\/receipts\/[0-9a-f-]{36}$/)
  const id = page.url().split('/').pop()!
  const receipt = await apiGet<{ lines: { product_id: string; qty: number; cost_tyiyn: number }[] }>(page.request, `/receipts/${id}`)
  expect(receipt.lines[0]).toMatchObject({ product_id: p.id, qty: 18_000, cost_tyiyn: 810_000 })
  expect((await apiGet<{ stock_qty: number }>(page.request, `/products/${p.id}`)).stock_qty).toBe(22_000)

  // «Исправить»: 18 л не делятся на канистры — строка открывается в литрах с ценой литра.
  await page.getByTestId('fix-button').click()
  await expect(page).toHaveURL(new RegExp(`/receipts/${id}/correct$`))
  await expect(page.getByTestId('line-1-mode-l')).toHaveAttribute('aria-checked', 'true')
  await expect(page.locator('#line-1-count')).toHaveValue('18')
  await expect(page.locator('#line-1-price')).toHaveValue('450')
  await page.locator('#line-1-count').fill('20')
  await page.locator('#correction-comment').fill('пришло 20 л')
  await page.getByRole('button', { name: 'Сохранить исправление' }).click()
  await expect(page).toHaveURL(/\/receipts\/[0-9a-f-]+$/)
  await expect(page).not.toHaveURL(new RegExp(id))
  const fixed = await apiGet<{ lines: { qty: number; cost_tyiyn: number }[] }>(page.request, `/receipts/${page.url().split('/').pop()!}`)
  expect(fixed.lines[0]).toMatchObject({ qty: 20_000, cost_tyiyn: 900_000 })
  expect((await apiGet<{ stock_qty: number }>(page.request, `/products/${p.id}`)).stock_qty).toBe(24_000)
})

test('оприходование масла — по умолчанию литрами, цена за литр', async ({ page }) => {
  await login(page)
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Масла оприходования ${RUN}`, kind: 'oil' })
  const code = `33${RUN}01`
  const p = await oil(page.request, cat.id, `Масло оприход ${RUN}`, code, 0)
  await page.goto('/receipts/new?kind=stock_in')
  await page.locator('#receipt-picker').fill(code)
  await page.locator('#receipt-picker').press('Enter')
  await expect(page.getByTestId('line-1-mode-l')).toHaveAttribute('aria-checked', 'true')
  await expect(page.locator('#line-1-count')).toHaveValue('4')
  await page.locator('#line-1-count').fill('6')
  await page.locator('#line-1-price').fill('450')
  await expect(page.getByTestId('receipt-total')).toHaveText(money('2 700 с'))
  await page.locator('#receipt-comment').fill(`остаток бочки ${RUN}`)
  await page.getByRole('button', { name: 'Провести оприходование' }).click()
  await expect(page.getByRole('heading', { name: /^Оприходование № \d+$/ })).toBeVisible()
  await expect(page.getByTestId('stock-in-total')).toHaveText(money('Итого: 2 700 с'))
  expect((await apiGet<{ stock_qty: number }>(page.request, `/products/${p.id}`)).stock_qty).toBe(6000)
})

test('«Сканер товаров» в пересчёте: масло — «кан. + л», итог в ревизии', async ({ page }) => {
  await login(page)
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Масла сканера ${RUN}`, kind: 'oil' })
  const code = `34${RUN}01`
  const p = await oil(page.request, cat.id, `Масло сканер ${RUN}`, code, 8000)
  await page.goto('/revision?tab=scan')
  const mode = page.getByLabel('Пересчёт (ревизия)')
  if (!(await mode.isChecked())) await mode.check()
  const scan = page.locator('[data-intake-scan]')
  await scan.fill(code)
  await scan.press('Enter')
  await expect(page.getByText('Уже заведён')).toBeVisible()
  await expect(page.getByTestId('oil-count').getByText(money('кан. по 4 л'))).toBeVisible()
  await page.locator('#intake-count').fill('1')
  await page.locator('#intake-count-l').fill('0,5')
  await expect(page.getByTestId('oil-count-total')).toHaveText(money('= 4,5 л'))
  await page.getByRole('button', { name: 'Записать' }).click()
  await expect(page.getByText(money('Записано в ревизию: всего 1 кан. + 0,5 л'))).toBeVisible()
  // Черновик «весь склад» не оставляем.
  const list = await apiGet<{ id: string; status: string; category_id: string | null }[]>(page.request, '/revisions')
  const draft = list.find((r) => r.status === 'draft' && r.category_id === null)!
  expect((await apiGet<{ counted_qty: number | null }>(page.request, `/revisions/${draft.id}/lines/${p.id}`)).counted_qty).toBe(4500)
  await apiPost(page.request, `/revisions/${draft.id}/cancel`, {})
  await mode.uncheck()
})
