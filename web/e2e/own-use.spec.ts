// Свой товар на нужды магазина (задача 038): окно «Мелкий расход из кассы» → «Свой товар» → товар,
// количество, «Списать» — остаток меньше, касса та же, в прибыли — «Товар на нужды магазина».
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e038' }
const RUN = Date.now().toString().slice(-8)

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' }).first()).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'GET' | 'POST', path: string, data?: unknown): Promise<T> {
  const res = await request.fetch(`/api/v1${path}`, { method, data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

type Product = { id: string; name: string; stock_qty: number }

test('свой товар на нужды магазина из окна мелкого расхода', async ({ page }) => {
  await login(page)
  const cat = await api<{ id: string }>(page.request, 'POST', '/categories', { name: `Нужды ${RUN}`, kind: 'other' })
  const name = `Тряпка нужд ${RUN}`
  const p = await api<Product>(page.request, 'POST', '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name, sale_price_tyiyn: 20_000 })
  await api(page.request, 'POST', '/stock-ins', { op_id: crypto.randomUUID(), comment: `нужды ${RUN}`, lines: [{ product_id: p.id, qty: 5, price_tyiyn: 10_000 }] })
  const cashBefore = (await api<{ balance_tyiyn: number }[]>(page.request, 'GET', '/cash/accounts')).reduce((s, a) => s + a.balance_tyiyn, 0)

  await page.goto('/')
  await page.getByRole('button', { name: /Мелкий расход из кассы/ }).first().click()
  const dialog = page.getByRole('dialog', { name: 'Мелкий расход из кассы' })
  await dialog.getByTestId('quick-mode-goods').click()
  await expect(dialog.getByTestId('quick-mode-goods')).toHaveAttribute('aria-pressed', 'true')
  const picker = dialog.getByPlaceholder('Сканируйте или найдите товар')
  await picker.fill(name)
  await expect(dialog.getByText(name).first()).toBeVisible()
  await picker.press('Enter')
  await expect(dialog.getByTestId('own-use-picked')).toContainText(name)
  await dialog.locator('#own-use-qty').fill('2')
  await dialog.locator('#own-use-comment').fill('протирать стёкла')
  await dialog.getByRole('button', { name: 'Списать' }).click()
  await expect(page.getByText(`Списано на нужды: ${name} — 2 шт`)).toBeVisible()
  await expect(dialog).toHaveCount(0)

  // Остаток 3, касса та же.
  const after = await api<Product>(page.request, 'GET', `/products/${p.id}`)
  expect(after.stock_qty).toBe(3)
  const cashAfter = (await api<{ balance_tyiyn: number }[]>(page.request, 'GET', '/cash/accounts')).reduce((s, a) => s + a.balance_tyiyn, 0)
  expect(cashAfter).toBe(cashBefore)
  // Прибыль: строка «Товар на нужды магазина» 200 с и документ в раскрытии.
  const profit = await api<{ totals: { own_use_tyiyn: number }; detail: { own_use: { product: string }[] } }>(page.request, 'GET', '/reports/profit?detail=1')
  expect(profit.totals.own_use_tyiyn).toBeGreaterThanOrEqual(20_000)
  expect(profit.detail.own_use.some((x) => x.product === name)).toBe(true)
  await page.goto('/profit')
  await page.getByRole('button', { name: /^Списание и брак/ }).click()
  await expect(page.getByRole('treeitem', { name: /Товар на нужды магазина/ }).first()).toBeVisible()
})
