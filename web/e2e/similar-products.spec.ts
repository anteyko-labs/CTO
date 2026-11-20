// «Похожие в базе» в карточке товара (задача 041 §5–7): похожий по артикулу виден при правке,
// его название правится прямо в строке, «Взять категорию» ставит его категорию этому товару.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000eb41' }
const RUN = Date.now().toString().slice(-8)

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'POST' | 'GET', path: string, data?: unknown): Promise<T> {
  const res = await request.fetch(`/api/v1${path}`, { method, data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

test('карточка товара: похожие в базе, правка в строке и «Взять категорию»', async ({ page }) => {
  await login(page)
  const right = await api<{ id: string }>(page.request, 'POST', '/categories', { name: `Масляные ${RUN}`, kind: 'filter' })
  const excel = await api<{ id: string }>(page.request, 'POST', '/categories', { name: `Из Excel ${RUN}`, kind: 'filter' })
  const mine = await api<{ id: string }>(page.request, 'POST', '/products', {
    op_id: crypto.randomUUID(),
    category_id: excel.id,
    name: `ФИЛЬТР W${RUN} из Excel`,
    article: `W${RUN}`,
    sale_price_tyiyn: 50_000,
  })
  await api(page.request, 'POST', '/products', {
    op_id: crypto.randomUUID(),
    category_id: right.id,
    name: `Масляный Mann ${RUN}`,
    article: `W-${RUN}`,
    sale_price_tyiyn: 60_000,
  })

  await page.goto('/products')
  const search = page.getByLabel('Поиск', { exact: true })
  await search.fill(`W${RUN}`)
  await search.press('Enter')
  const card = page.getByRole('dialog', { name: 'Товар' })
  await expect(card.locator('#product-name')).toHaveValue(`ФИЛЬТР W${RUN} из Excel`)
  const block = card.getByTestId('product-similar')
  const row = block.getByTestId('similar-row')
  await expect(row).toHaveCount(1)
  await expect(row).toContainText('тот же код')

  await row.getByLabel('Название похожего').fill(`Масляный Mann W${RUN}`)
  await row.getByRole('button', { name: 'Сохранить' }).click()
  await expect(row.getByRole('button', { name: 'Сохранить' })).toHaveCount(0)

  await row.getByRole('button', { name: 'Взять категорию' }).click()
  await expect(card.locator('#product-category')).toHaveValue(right.id)
  const p = await api<{ category_id: string }>(page.request, 'GET', `/products/${mine.id}`)
  expect(p.category_id).toBe(right.id)
  const similar = await api<{ name: string }[]>(page.request, 'GET', `/products/${mine.id}/similar`)
  expect(similar.map((s) => s.name)).toEqual([`Масляный Mann W${RUN}`])
})
