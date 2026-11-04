// Метры у любого товара (задача 006, ADR-178): товар заводится в метрах из прихода, приход 3,5 м по
// цене за метр, в ревизии вписывают «2» — это 2 м, а не 2 мм; после проведения остаток 2 000 мм.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { discardEntryDrafts, ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e6a2' }
const RUN = Date.now().toString().slice(-8)

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'get' | 'post', path: string, data?: unknown): Promise<T> {
  const res = await request[method](`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

test('товар в метрах: завести в приходе, приход 3,5 м, ревизия «2» — остаток 2 м', async ({ page }) => {
  await login(page)
  await discardEntryDrafts(page.request)
  const category = `Кат м ${RUN}`
  const cat = await api<{ id: string }>(page.request, 'post', '/categories', { name: category, kind: 'other' })
  const name = `Шланг м ${RUN}`

  // Новый товар из прихода — «Учитывать: в метрах», цена за метр.
  await page.goto('/receipts/new')
  await page.locator('#receipt-picker').fill(name)
  await page.getByRole('button', { name: `+ Завести товар «${name}»` }).click()
  const dialog = page.getByRole('dialog', { name: 'Новый товар' })
  await dialog.locator('#product-category').selectOption(cat.id)
  await dialog.getByTestId('product-unit').selectOption('mm')
  await expect(dialog.getByText('Цена продажи за м, с')).toBeVisible()
  await dialog.locator('#product-price').fill('300')
  await dialog.getByRole('button', { name: 'Сохранить' }).click()
  await expect(dialog).toBeHidden()
  const found = await api<{ id: string; unit: string }[]>(page.request, 'get', `/products?q=${encodeURIComponent(name)}`)
  expect(found[0]?.unit).toBe('mm')
  const id = found[0].id

  // Приход 3,5 м по 200 с за метр.
  await page.locator('#line-1-count').fill('3,5')
  await page.locator('#line-1-price').fill('200')
  await page.locator('#receipt-paid-from').selectOption(await ownerSafe(page.request))
  await page.getByRole('button', { name: 'Провести приход' }).first().click()
  await expect(page).toHaveURL(/\/receipts\/[0-9a-f-]{36}$/)
  const after = await api<{ stock_qty: number }>(page.request, 'get', `/products/${id}`)
  expect(after.stock_qty).toBe(3_500)

  // Ревизия категории: на полке 2 м.
  await page.goto('/revision')
  await page.getByLabel('Что пересчитываем').selectOption({ label: category })
  await page.getByRole('button', { name: 'Начать ревизию' }).click()
  await expect(page.getByRole('heading', { name: /Ревизия № \d+/ })).toBeVisible()
  const field = page.locator(`[data-count="${id}"]`)
  await field.fill('2')
  await field.press('Enter')
  await expect(page.locator(`[data-total="${id}"]`)).toHaveText(/итого\s2\sм/, { timeout: 15_000 })
  await page.getByRole('button', { name: 'Провести' }).click()
  await page.getByLabel(/Кто пересчитывал/).fill('Айбек, м')
  await page.getByRole('button', { name: 'Провести' }).last().click()
  await expect(page.getByText('Ревизия проведена')).toBeVisible()
  const counted = await api<{ stock_qty: number }>(page.request, 'get', `/products/${id}`)
  expect(counted.stock_qty).toBe(2_000)
})
