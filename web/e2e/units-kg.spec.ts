// Килограммы у любого товара (ADR-174): товар заводится в кг прямо из прихода, приход 2,5 кг по
// цене за кг, в ревизии вписывают 1,25 кг — после проведения остаток 1 250 г.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e6a1' }
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

test('товар в кг: завести в приходе, приход 2,5 кг, ревизия 1,25 кг — остаток 1 250 г', async ({ page }) => {
  await login(page)
  const category = `Кат кг ${RUN}`
  const cat = await api<{ id: string }>(page.request, 'post', '/categories', { name: category, kind: 'other' })
  const name = `Солидол кг ${RUN}`

  // Новый товар из прихода — «Учитывать: в килограммах», цена за кг.
  await page.goto('/receipts/new')
  await page.locator('#receipt-picker').fill(name)
  await page.getByRole('button', { name: `+ Завести товар «${name}»` }).click()
  const dialog = page.getByRole('dialog', { name: 'Новый товар' })
  await dialog.locator('#product-category').selectOption(cat.id)
  await dialog.getByTestId('product-unit').selectOption('g')
  await expect(dialog.getByText('Цена продажи за кг, с')).toBeVisible()
  await dialog.locator('#product-price').fill('300')
  await dialog.getByRole('button', { name: 'Сохранить' }).click()
  await expect(dialog).toBeHidden()
  const found = await api<{ id: string; unit: string }[]>(page.request, 'get', `/products?q=${encodeURIComponent(name)}`)
  expect(found[0]?.unit).toBe('g')
  const id = found[0].id

  // Приход 2,5 кг по 200 с за кг.
  await page.locator('#line-1-count').fill('2,5')
  await page.locator('#line-1-price').fill('200')
  await page.locator('#receipt-paid-from').selectOption(await ownerSafe(page.request))
  await page.getByRole('button', { name: 'Провести приход' }).first().click()
  await expect(page).toHaveURL(/\/receipts\/[0-9a-f-]{36}$/)
  const after = await api<{ stock_qty: number }>(page.request, 'get', `/products/${id}`)
  expect(after.stock_qty).toBe(2_500)

  // Ревизия категории: на полке 1,25 кг.
  await page.goto('/revision')
  await page.getByLabel('Что пересчитываем').selectOption({ label: category })
  await page.getByRole('button', { name: 'Начать ревизию' }).click()
  await expect(page.getByRole('heading', { name: /Ревизия № \d+/ })).toBeVisible()
  const field = page.locator(`[data-count="${id}"]`)
  await field.fill('1,25')
  await field.press('Enter')
  await expect(page.locator(`[data-total="${id}"]`)).toHaveText(/итого\s1,25\sкг/, { timeout: 15_000 })
  await page.getByRole('button', { name: 'Провести' }).click()
  await page.getByLabel(/Кто пересчитывал/).fill('Айбек, кг')
  await page.getByRole('button', { name: 'Провести' }).last().click()
  await expect(page.getByText('Ревизия проведена')).toBeVisible()
  const counted = await api<{ stock_qty: number }>(page.request, 'get', `/products/${id}`)
  expect(counted.stock_qty).toBe(1_250)
})
