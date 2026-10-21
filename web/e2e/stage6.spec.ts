// Брак и поставки в браузере (SPEC-25, SPEC-26): отметить брак из остатков, решить его
// возвратом поставщику со скидкой и увидеть возврат в карточке поставщика.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2e6' }
const RUN = Date.now().toString().slice(-8)

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

test('брак: отметка, возврат поставщику со скидкой, поставки в карточке', async ({ page }) => {
  const name = `Фильтр брак ${RUN}`
  await login(page)
  const category = await apiPost<{ id: string }>(page.request, '/categories', { name: `Брак ${RUN}`, kind: 'filter' })
  const product = await apiPost<{ id: string }>(page.request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: category.id,
    name,
    sale_price_tyiyn: 50_000,
  })
  const supplier = await apiPost<{ id: string }>(page.request, '/suppliers', { name: `Автоснаб ${RUN}` })
  await apiPost(page.request, '/receipts', {
    op_id: crypto.randomUUID(),
    supplier_id: supplier.id,
    payment: 'paid',
    paid_from: await ownerSafe(page.request),
    lines: [{ product_id: product.id, qty: 5, cost_tyiyn: 150_000 }],
  })

  // «Товары» (бывшие «Остатки», SPEC-30 §11) → «В брак»: поставщик подставлен из прихода.
  await page.goto('/products')
  await page.getByLabel('Поиск', { exact: true }).fill(name)
  const row = page.getByRole('row', { name: new RegExp(name) })
  await row.getByRole('button', { name: 'Ещё действия' }).click()
  await page.getByRole('menuitem', { name: 'В брак' }).click()
  const dialog = page.getByRole('dialog', { name: `В брак · ${name}` })
  await expect(dialog.getByLabel('Поставщик')).toHaveValue(supplier.id)
  await dialog.getByLabel('Количество, шт').fill('2')
  await dialog.getByLabel('Причина').fill('битый корпус')
  await dialog.getByRole('button', { name: 'В брак' }).click()
  await expect(dialog).toBeHidden()
  await expect(row.getByText(/^3\sшт$/)).toBeVisible()

  // Вкладка «Брак»: решение владельца — скидка на следующий приход.
  await page.getByRole('tab', { name: 'Брак' }).click()
  const pending = page.getByRole('row', { name: new RegExp(name) })
  await expect(pending.getByText('600 с')).toBeVisible()
  await pending.getByRole('button', { name: 'Решить' }).click()
  const resolve = page.getByRole('dialog', { name: /Решение по браку/ })
  await resolve.getByRole('radio', { name: 'Вернуть → скидка / из долга' }).click()
  await resolve.getByLabel('Сумма возмещения, с').fill('500')
  await expect(resolve.getByText(/убыток 100\sс/)).toBeVisible()
  await resolve.getByRole('button', { name: 'Провести' }).click()
  await expect(resolve).toBeHidden()
  await page.getByRole('button', { name: 'Решённые' }).click()
  await expect(page.getByRole('row', { name: new RegExp(name) }).getByText(/скидка \/ из долга · возмещение 500\sс/)).toBeVisible()

  // Карточка поставщика: вернули 2 шт, он должен нам 500 с, на складе 3 шт — все от него.
  await page.goto(`/suppliers/${supplier.id}`)
  await expect(page.getByText(/он должен нам 500\sс/)).toBeVisible()
  const delivery = page.getByRole('row', { name: new RegExp(name) })
  await expect(delivery.getByRole('cell').nth(1)).toHaveText(/5\sшт/)
  await expect(delivery.getByRole('cell').nth(2)).toHaveText(/2\sшт/)
  await expect(delivery.getByRole('cell').nth(6)).toHaveText(/3\sшт/)
})
