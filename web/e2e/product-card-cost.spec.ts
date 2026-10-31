// Цена прихода нового товара и себестоимость в карточке (задача 003, Н7, ADR-150): новый товар
// с ценой прихода «за кан. | за л» из поиска в приходе — строка получает цену и режим; владельцу в
// карточке — средняя за канистру и за литр, стоимость остатка, наценка, «Последний приход»;
// администратору — только «Последний приход» (I13, ADR-040); товар без закупки — «Указать цену закупки».
import { expect, test, type APIRequestContext, type Browser, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e3c7' }
const RUN = Date.now().toString().slice(-8)
const money = (text: string) => new RegExp(text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&').replace(/ /g, '\\s'))

async function login(page: Page, user = OWNER, password = OWNER_PASSWORD) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(user)
  await page.getByLabel('Пароль').fill(password)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function adminPage(browser: Browser, user: string, password: string) {
  const ctx = await browser.newContext()
  const page = await ctx.newPage()
  await login(page, user, password)
  return { ctx, page }
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

const openCard = async (page: Page, name: string) => {
  await page.goto('/products')
  await page.getByLabel('Поиск', { exact: true }).fill(name)
  const row = page.getByRole('row', { name: new RegExp(name) })
  await expect(row).toBeVisible()
  await row.getByText(name).first().click()
  return page.getByRole('dialog', { name: 'Товар' })
}

test('новый товар в приходе: цена прихода за литр уходит в строку литрами', async ({ page }) => {
  await login(page)
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Масла карточки ${RUN}`, kind: 'oil' })
  const name = `Масло карточка ${RUN} 4L`
  await page.goto('/receipts/new')
  await page.locator('#receipt-picker').fill(name)
  await page.getByRole('button', { name: `+ Завести товар «${name}»` }).click()
  const dialog = page.getByRole('dialog', { name: 'Новый товар' })
  await dialog.locator('#product-category').selectOption(cat.id)
  await dialog.locator('#product-container').fill('4')
  await dialog.locator('#product-price').fill('2000')
  // По умолчанию в приходе — за канистру; переключаем на литр и вводим цену литра.
  await expect(dialog.getByTestId('product-cost-per-can')).toHaveAttribute('aria-checked', 'true')
  await dialog.getByTestId('product-cost-per-l').click()
  await dialog.locator('#product-cost').fill('450')
  await expect(dialog.getByTestId('product-cost-hint')).toContainText(money('= 1 800 с за канистру 4 л'))
  await dialog.locator('#product-cost').fill('-1')
  await expect(dialog.getByText('неверная цена')).toBeVisible()
  await expect(dialog.getByRole('button', { name: 'Сохранить' })).toBeDisabled()
  await dialog.locator('#product-cost').fill('450')
  await dialog.getByRole('button', { name: 'Сохранить' }).click()
  await expect(dialog).toBeHidden()
  await expect(page.getByTestId('line-1-mode-l')).toHaveAttribute('aria-checked', 'true')
  await expect(page.locator('#line-1-count')).toHaveValue('4')
  await expect(page.locator('#line-1-price')).toHaveValue('450')
  await expect(page.getByTestId('liters-per-can')).toHaveText(money('за канистру 4 л ≈ 1 800 с'))
  const found = await apiGet<{ last_purchase_price_tyiyn: number | null }[]>(page.request, `/products?q=${encodeURIComponent(name)}`)
  expect(found[0]?.last_purchase_price_tyiyn).toBe(180_000)
})

test('себестоимость в карточке: владельцу — средняя и остаток, администратору — только последний приход', async ({ page, browser }) => {
  await login(page)
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Масла себес ${RUN}`, kind: 'oil' })
  const name = `Масло себес ${RUN}`
  const p = await apiPost<{ id: string }>(page.request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name,
    container_ml: 4000,
    sale_price_tyiyn: 200_000,
  })
  const supplier = await apiPost<{ id: string }>(page.request, '/suppliers', { name: `Автоснаб себес ${RUN}` })
  const receipt = await apiPost<{ id: string; number: number }>(page.request, '/receipts', {
    op_id: crypto.randomUUID(),
    supplier_id: supplier.id,
    payment: 'debt',
    lines: [{ product_id: p.id, qty: 8000, cost_tyiyn: 360_000 }],
  })

  const card = await openCard(page, name)
  const cost = card.getByTestId('product-cost')
  await expect(cost).toContainText('Себестоимость')
  await expect(card.getByTestId('product-cost-avg')).toHaveText(money('1 800 с за канистру 4 л · 450 с за л'))
  await expect(card.getByTestId('product-cost-stock')).toHaveText(money('2 кан. — 3 600 с'))
  await expect(card.getByTestId('product-cost-markup')).toHaveText(money('цена продажи 2 000 с — наценка 11,1 % к средней'))
  await expect(card.getByTestId('product-last-receipt')).toContainText(money(`1 800 с за канистру 4 л · 450 с за л`))
  await expect(card.getByTestId('product-last-receipt')).toContainText(`Автоснаб себес ${RUN}`)
  await expect(card.getByTestId('product-last-receipt').getByRole('link', { name: `приход № ${receipt.number}` })).toBeVisible()
  await expect(card.getByRole('button', { name: 'Указать цену закупки' })).toHaveCount(0)

  // Администратор: сервер не отдаёт среднюю и стоимость остатка — блока «Себестоимость» нет.
  const admin = `sebes${RUN}`
  const password = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  const res = await page.request.post('/api/v1/users', {
    data: { login: admin, password, full_name: `Админ себес ${RUN}`, role: 'admin' },
    headers: DEVICE,
  })
  expect(res.ok(), await res.text()).toBeTruthy()
  const { ctx, page: a } = await adminPage(browser, admin, password)
  const adminCard = await openCard(a, name)
  await expect(adminCard.getByTestId('product-last-receipt')).toContainText(money('1 800 с за канистру 4 л'))
  await expect(adminCard.getByTestId('product-cost-owner')).toHaveCount(0)
  await expect(adminCard.getByText('Себестоимость')).toHaveCount(0)
  const raw = await apiGet<Record<string, unknown>>(a.request, `/products/${p.id}`)
  expect(raw.avg_cost_tyiyn).toBeUndefined()
  expect(raw.stock_value_tyiyn).toBeUndefined()
  await ctx.close()
})

test('товар без закупки: «Указать цену закупки» из карточки, за литр', async ({ page }) => {
  await login(page)
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Масла без цены ${RUN}`, kind: 'oil' })
  const name = `Масло без цены ${RUN}`
  await apiPost(page.request, '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name, container_ml: 4000, sale_price_tyiyn: 200_000 })
  const card = await openCard(page, name)
  await expect(card.getByTestId('product-cost-note')).toHaveText('остатка нет и цены закупки нет')
  await expect(card.getByTestId('product-last-receipt')).toContainText('цены закупки нет')
  await card.getByRole('button', { name: 'Указать цену закупки' }).click()
  const modal = page.getByRole('dialog', { name: `Цена закупки · ${name}` })
  await modal.getByTestId('purchase-price-per-l').click()
  await modal.locator('#purchase-price').fill('425')
  await expect(modal.getByTestId('purchase-price-hint')).toContainText(money('= 1 700 с за канистру 4 л'))
  await modal.getByRole('button', { name: 'Сохранить' }).click()
  await expect(modal).toBeHidden()
  await expect(card.getByTestId('product-last-receipt')).toContainText(money('начальная цена закупки: 1 700 с за канистру 4 л · 425 с за л'))
  await expect(card.getByTestId('product-cost-note')).toHaveText('остатка нет — продажа пойдёт по последней цене прихода')
})
