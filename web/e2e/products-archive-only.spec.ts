// «Показать архив» в «Товарах» — только товары в архиве (задача 020): товар в продаже пропадает,
// архивный появляется с отметкой «архив»; снять отметку — снова только товары в продаже.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e020' }
const RUN = Date.now().toString().slice(-8)

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'post' | 'patch', path: string, data: unknown): Promise<T> {
  const res = await request[method](`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

test('«Показать архив» — в списке только товары в архиве', async ({ page }) => {
  await login(page)
  const cat = await api<{ id: string }>(page.request, 'post', '/categories', { name: `Категория ${RUN}`, kind: 'other' })
  const active = `Товар в продаже ${RUN}`
  const archived = `Товар в архиве ${RUN}`
  for (const name of [active, archived]) {
    const p = await api<{ id: string }>(page.request, 'post', '/products', {
      op_id: crypto.randomUUID(),
      category_id: cat.id,
      name,
      sale_price_tyiyn: 10_000,
    })
    if (name === archived) await api(page.request, 'patch', `/products/${p.id}`, { archived: true })
  }

  await page.goto('/products')
  await page.getByLabel('Поиск', { exact: true }).fill(RUN)
  const activeRow = page.getByRole('row', { name: new RegExp(active) })
  const archivedRow = page.getByRole('row', { name: new RegExp(archived) })
  await expect(activeRow).toBeVisible()
  await expect(archivedRow).toHaveCount(0)

  await page.getByLabel('Показать архив').check()
  await expect(archivedRow).toBeVisible()
  await expect(archivedRow.getByText('архив', { exact: true })).toBeVisible()
  await expect(activeRow).toHaveCount(0)

  await page.getByLabel('Показать архив').uncheck()
  await expect(activeRow).toBeVisible()
  await expect(archivedRow).toHaveCount(0)
})
