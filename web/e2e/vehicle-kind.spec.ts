// «Грузовая / Легковая» на кассе (задача 040 A, ADR-205): новый чек начинается со значения кассы
// филиала, кассир переключает на этот чек, после проведения — снова значение филиала. В конце —
// настройки как были.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ensureShift, openPay } from './cashier'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e040' }
const RUN = Date.now().toString().slice(-8)
const PICKER = /Сканируйте штрихкод/

test.use({ viewport: { width: 1366, height: 768 } })

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'GET' | 'POST' | 'PUT', path: string, data?: unknown): Promise<T> {
  const res = await request.fetch(`/api/v1${path}`, { method, data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

test('ползунок «Грузовая / Легковая»: значение филиала, своё на чек, после чека — снова филиала', async ({ page }) => {
  await login(page)
  const request = page.request
  const before = await api<Record<string, unknown>>(request, 'GET', '/settings/cashier')
  await api(request, 'PUT', '/settings/cashier', { ...before, op_id: crypto.randomUUID(), vehicle_kind_default: 'truck' })
  try {
    const cat = await api<{ id: string }>(request, 'POST', '/categories', { name: `Грузовые ${RUN}`, kind: 'other' })
    const name = `Щётка ${RUN}`
    const p = await api<{ id: string }>(request, 'POST', '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name, sale_price_tyiyn: 50_000 })
    await api(request, 'POST', '/receipts', {
      op_id: crypto.randomUUID(),
      paid_from: await ownerSafe(request),
      lines: [{ product_id: p.id, qty: 5, cost_tyiyn: 100_000 }],
    })
    await ensureShift(request)

    await page.goto('/')
    const truck = page.getByTestId('vehicle-kind-truck')
    const car = page.getByTestId('vehicle-kind-car')
    await expect(truck).toHaveAttribute('aria-checked', 'true')
    await expect(car).toHaveAttribute('aria-checked', 'false')
    await car.click()
    await expect(car).toHaveAttribute('aria-checked', 'true')

    await page.getByPlaceholder(PICKER).fill(name)
    await page.getByRole('button', { name: new RegExp(`^${name}`) }).first().click()
    await expect(page.getByTestId('cart')).toContainText(name)
    await openPay(page)
    await page.getByRole('button', { name: 'Провести чек' }).first().click()
    await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()
    const href = (await page.getByRole('link', { name: 'Открыть' }).first().getAttribute('href')) ?? ''
    const sale = await api<{ vehicle_kind: string }>(request, 'GET', href)
    expect(sale.vehicle_kind).toBe('car')
    // Новый чек — снова значение филиала.
    await expect(truck).toHaveAttribute('aria-checked', 'true')
  } finally {
    await api(request, 'PUT', '/settings/cashier', { ...before, op_id: crypto.randomUUID() })
  }
})
