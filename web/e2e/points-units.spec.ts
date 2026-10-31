// Задача 002, волна 2: баллы в карточке товара и категории, выключатель «только в чеке с заменой
// масла» (П1); единица и объём канистры товара с остатком — одной операцией (П4; задача 003, Н10).
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e076' }
const RUN = Date.now().toString().slice(-8)

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'get' | 'post' | 'put', path: string, data?: unknown): Promise<T> {
  const res = await request[method](`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

interface Product {
  id: string
  unit: string
  container_ml: number | null
  stock_qty: number
  points_mode: string
  points_value: number
}

/** Масло в канистрах по 4 л: три канистры на складе. */
async function oil(request: APIRequestContext, tag: string) {
  const cat = await api<{ id: string }>(request, 'post', '/categories', { name: `Масла ${tag} ${RUN}`, kind: 'oil' })
  const name = `Масло ${tag} ${RUN} 4л`
  const p = await api<{ id: string }>(request, 'post', '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name,
    container_ml: 4000,
    sale_price_tyiyn: 200_000,
  })
  const supplier = await api<{ id: string }>(request, 'post', '/suppliers', { name: `Поставщик масла ${tag} ${RUN}` })
  await api(request, 'post', '/receipts', {
    op_id: crypto.randomUUID(),
    supplier_id: supplier.id,
    payment: 'debt',
    lines: [{ product_id: p.id, qty: 12_000, cost_tyiyn: 600_000 }],
  })
  return { name, id: p.id, catName: `Масла ${tag} ${RUN}` }
}

async function openCard(page: Page, name: string) {
  await page.goto('/products')
  await page.getByPlaceholder('Название, бренд, артикул, штрихкод').fill(name)
  await page.getByText(name).first().click()
  const card = page.getByRole('dialog', { name: 'Товар' })
  await expect(card).toBeVisible()
  return card
}

test('баллы товара: правило в карточке сохраняется, по умолчанию — как в категории', async ({ page }) => {
  await login(page)
  const w = await oil(page.request, 'баллы')
  const card = await openCard(page, w.name)
  await expect(card.getByTestId('points-mode')).toHaveValue('inherit')
  await card.getByTestId('points-mode').selectOption('fixed')
  await card.getByTestId('points-value').fill('10')
  await card.getByRole('button', { name: 'Сохранить' }).click()
  await expect(card).toBeHidden()
  const p = await api<Product>(page.request, 'get', `/products/${w.id}`)
  expect([p.points_mode, p.points_value]).toEqual(['fixed', 1000])

  // Категория: 2 % от оплаченного для её товаров.
  await page.goto('/categories')
  await page.getByText(w.catName).first().click()
  const cat = page.getByRole('dialog', { name: 'Категория' })
  await cat.getByTestId('points-mode').selectOption('percent')
  await cat.getByTestId('points-value').fill('2')
  await cat.getByRole('button', { name: 'Сохранить' }).click()
  await expect(cat).toBeHidden()
  const cats = await api<{ id: string; name: string; points_mode: string; points_value: number }[]>(page.request, 'get', '/categories')
  const saved = cats.find((c) => c.name === w.catName)!
  expect([saved.points_mode, saved.points_value]).toEqual(['percent', 200])
})

test('объём канистры с остатком: 3 канистры по 4 л → по 5 л одной операцией; потом в штуки', async ({ page }) => {
  await login(page)
  const w = await oil(page.request, 'канистра')
  let card = await openCard(page, w.name)
  await expect(card.getByTestId('product-unit-text')).toContainText('канистра 4')
  await card.getByTestId('product-unit-change').click()
  const modal = page.getByTestId('unit-modal')
  await modal.getByTestId('unit-container').fill('5')
  await expect(modal.getByTestId('unit-preview')).toContainText('12')
  await expect(modal.getByTestId('unit-preview')).toContainText('15')
  await modal.getByTestId('unit-save').click()
  await expect(modal).toBeHidden()
  let p = await api<Product>(page.request, 'get', `/products/${w.id}`)
  expect([p.unit, p.container_ml, p.stock_qty]).toEqual(['ml', 5000, 15_000])

  // В штуки по 0,4 л — 15 л не делятся: кнопка закрыта, подсказка; по 5 л — 3 шт.
  card = await openCard(page, w.name)
  await card.getByTestId('product-unit-change').click()
  await modal.getByTestId('unit-target').selectOption('piece')
  await modal.getByTestId('unit-piece').fill('0,4')
  await expect(modal.getByText(/не переходит в целое число/)).toBeVisible()
  await expect(modal.getByTestId('unit-save')).toBeDisabled()
  await modal.getByTestId('unit-piece').fill('5')
  await expect(modal.getByTestId('unit-preview')).toContainText('3 шт')
  await modal.getByTestId('unit-save').click()
  await expect(modal).toBeHidden()
  p = await api<Product>(page.request, 'get', `/products/${w.id}`)
  expect([p.unit, p.container_ml, p.stock_qty]).toEqual(['piece', null, 3])
})

test('«только в чеке с заменой масла» — выключатель в «Телеграм и баллы»', async ({ page }) => {
  await login(page)
  const before = await api<{ rate_bp: number; oil_change_only: boolean }>(page.request, 'get', '/settings/loyalty')
  await page.goto('/settings/telegram')
  const box = page.getByTestId('loyalty-oil-only').getByRole('checkbox')
  // Бот на стенде может быть выключен — тогда карточки баллов нет; настройка проверяется по API.
  if ((await box.count()) === 0) {
    await api(page.request, 'put', '/settings/loyalty', { rate_bp: before.rate_bp, oil_change_only: !before.oil_change_only })
  } else {
    await expect(box).toBeChecked({ checked: before.oil_change_only })
    await box.click()
    await expect(box).toBeChecked({ checked: !before.oil_change_only })
  }
  const after = await api<{ oil_change_only: boolean }>(page.request, 'get', '/settings/loyalty')
  expect(after.oil_change_only).toBe(!before.oil_change_only)
  await api(page.request, 'put', '/settings/loyalty', before)
})
