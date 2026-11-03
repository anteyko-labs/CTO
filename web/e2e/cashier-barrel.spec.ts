// Бочка на кассе (задача 010): товар в литрах с тарой больше порога (150 л) — в чек сразу
// «Розлив, л», фокус в литрах, сумма по цене литра, чек проводится; канистра 4 л — как прежде;
// порог 250 в настройках — бочка 200 л снова канистрой. В конце — настройки как были.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { openPay } from './cashier'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e7b1' }
const RUN = Date.now().toString().slice(-8)
const PICKER = /Сканируйте штрихкод/
const money = (s: string) => new RegExp(s.replace(/ /g, '\\s'))

test.use({ viewport: { width: 1366, height: 768 } })

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'POST' | 'PUT', path: string, data: unknown): Promise<T> {
  const res = await request.fetch(`/api/v1${path}`, { method, data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

const cashier = (request: APIRequestContext, over?: number) =>
  api(request, 'PUT', '/settings/cashier', { op_id: crypto.randomUUID(), ...(over === undefined ? {} : { barrel_pour_over_l: over }) })

async function addToCart(page: Page, name: string) {
  await page.getByPlaceholder(PICKER).fill(name)
  await page.getByRole('button', { name: new RegExp(`^${name}`) }).first().click()
  await expect(page.getByTestId('cart')).toContainText(name)
}

test('бочка 200 л — розлив литрами, канистра 4 л — канистрой, порог из настроек', async ({ page }) => {
  await login(page)
  const request = page.request
  await cashier(request)
  const cat = await api<{ id: string }>(request, 'POST', '/categories', { name: `Бочки ${RUN}`, kind: 'oil' })
  const barrel = `Бочка ${RUN} 10W-40`
  const can = `Канистра ${RUN} 5W-30`
  // Бочка 200 л по 20 000 с — 100 с за литр.
  const b = await api<{ id: string }>(request, 'POST', '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name: barrel, container_ml: 200_000, sale_price_tyiyn: 2_000_000 })
  const c = await api<{ id: string }>(request, 'POST', '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name: can, container_ml: 4_000, sale_price_tyiyn: 300_000 })
  await api(request, 'POST', '/receipts', {
    op_id: crypto.randomUUID(),
    paid_from: await ownerSafe(request),
    lines: [
      { product_id: b.id, qty: 200_000, cost_tyiyn: 1_000_000 },
      { product_id: c.id, qty: 40_000, cost_tyiyn: 1_000_000 },
    ],
  })
  const cur = (await (await request.get('/api/v1/shifts/current')).json()) as { id: string } | null
  if (!cur) {
    const emp = await api<{ id: string }>(request, 'POST', '/employees', { full_name: `Кассир бочки ${RUN}`, is_cashier: true, pin: '1597' })
    await api(request, 'POST', '/shifts/open', { op_id: crypto.randomUUID(), cashier_employee_id: emp.id, cashier_pin: '1597' })
  }

  await page.goto('/')
  await addToCart(page, barrel)
  const liters = page.getByLabel('Литры')
  await expect(liters).toBeFocused()
  await expect(liters).toHaveValue('')
  await expect(page.getByRole('button', { name: 'Розлив, л' })).toHaveClass(/bg-sky-600/)
  await liters.fill('4,5')
  await addToCart(page, can)
  await expect(page.getByLabel('Канистры')).toHaveValue('1')
  // 4,5 л × 100 с + канистра 3 000 с.
  await openPay(page)
  await expect(page.getByTestId('pay-top')).toContainText(money('3 450 с'))
  await page.getByRole('button', { name: 'Провести чек' }).first().click()
  await page.getByRole('button', { name: 'Нет, на вынос' }).click()
  await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()

  // Порог 250 л: бочка 200 л — снова канистрой.
  await cashier(request, 250)
  await page.reload()
  await addToCart(page, barrel)
  await expect(page.getByLabel('Канистры')).toHaveValue('1')
  await expect(page.getByLabel('Литры')).toHaveCount(0)
  await cashier(request)
})
