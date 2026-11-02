// Задача 007: этикетки по накладной — список позиций, у каждой число (по умолчанию сколько
// пришло), число правится, 0 — не печатать.
import { expect, test, type Page } from '@playwright/test'

const RUN = Date.now().toString().slice(-8)
const H = { 'X-Device-Id': '00000000-0000-4000-8000-0000000ab1e1' }

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(process.env.E2E_LOGIN ?? 'owner')
  await page.getByLabel('Пароль').fill(process.env.E2E_PASSWORD ?? 'owner-pass-1')
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' }).first()).toBeVisible()
}

test('оприходование → «Печатать этикетки»: список позиций с числом, правится перед печатью', async ({ page }) => {
  await login(page)
  const post = async <T,>(path: string, data: unknown): Promise<T> => {
    const r = await page.request.post(`/api/v1${path}`, { data, headers: H })
    expect(r.ok(), await r.text()).toBeTruthy()
    return (await r.json()) as T
  }
  const cat = await post<{ id: string }>('/categories', { name: `Этикетки приход ${RUN}`, kind: 'other' })
  const a = await post<{ id: string }>('/products', { op_id: crypto.randomUUID(), category_id: cat.id, name: `Свеча А ${RUN}`, sale_price_tyiyn: 10_000 })
  const b = await post<{ id: string }>('/products', { op_id: crypto.randomUUID(), category_id: cat.id, name: `Свеча Б ${RUN}`, sale_price_tyiyn: 20_000 })
  const doc = await post<{ id: string }>('/stock-ins', {
    op_id: crypto.randomUUID(),
    comment: 'e2e',
    lines: [
      { product_id: a.id, qty: 5, price_tyiyn: 5_000, unit: 'piece', container_ml: null },
      { product_id: b.id, qty: 2, price_tyiyn: 9_000, unit: 'piece', container_ml: null },
    ],
  })
  // Как после проведения с «Печатать этикетки»: список открывается сам.
  await page.goto(`/stock-ins/${doc.id}?labels=1`)
  const list = page.getByTestId('label-list')
  await expect(list.locator(`[data-label-copies="${a.id}"]`)).toHaveValue('5')
  await expect(list.locator(`[data-label-copies="${b.id}"]`)).toHaveValue('2')
  await list.locator(`[data-label-copies="${a.id}"]`).fill('3')
  await list.locator(`[data-label-copies="${b.id}"]`).fill('0')
  await expect(list.getByRole('button', { name: 'Печатать 3' })).toBeEnabled()
  // Печать окном браузера в тесте не нужна — подменяем.
  await page.evaluate(() => (window.print = () => undefined))
  await list.getByRole('button', { name: 'Печатать 3' }).click()
  await expect(list).toBeHidden()
})
