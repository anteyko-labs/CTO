// Процент кассирам по ждущим долговым чекам — разом (задача 054): «Настройки → Касса» показывает
// ждущие чеки и итог; без «Процента с долга — сразу» кнопка недоступна; после начисления блок пропадает.
// Файл с «z-» — идёт последним: начисление закрывает все ждущие проценты общей базы e2e.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { SIGNATURE_PNG } from './sign'
import { ownerSafe } from './safe'
import { ensureShift } from './cashier'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e054' }
const RUN = Date.now().toString().slice(-8)

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' }).first()).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'GET' | 'POST' | 'PUT', path: string, data?: unknown): Promise<T> {
  const res = await request.fetch(`/api/v1${path}`, { method, data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

test('процент по ждущим долгам — разом, после включения настройки', async ({ page }) => {
  await login(page)
  const request = page.request
  const legal = await api<{ approval_mode: string; approval_timeout_min: number }>(request, 'GET', '/settings/legal')
  await api(request, 'PUT', '/settings/legal', { op_id: crypto.randomUUID(), approval_mode: 'off', approval_timeout_min: legal.approval_timeout_min })
  await api(request, 'PUT', '/settings/cashier', { op_id: crypto.randomUUID(), debt_percent_now: false })
  try {
    const shift = await ensureShift(request)
    const cat = await api<{ id: string }>(request, 'POST', '/categories', { name: `Долги ${RUN}`, kind: 'other' })
    const p = await api<{ id: string }>(request, 'POST', '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name: `Долги ${RUN} фильтр`, article: '', sale_price_tyiyn: 100_000, min_stock: 0 })
    await api(request, 'POST', '/receipts', { op_id: crypto.randomUUID(), paid_from: await ownerSafe(request), lines: [{ product_id: p.id, qty: 2, cost_tyiyn: 120_000 }] })
    const firm = await api<{ id: string }>(request, 'POST', '/parties', { role: 'customer', kind: 'company', name: `ОсОО Долги ${RUN}`, phone: `0701${RUN.slice(-6)}`, inn: `0${RUN}54321` })
    await api(request, 'POST', '/sales', {
      op_id: crypto.randomUUID(),
      client_time: new Date().toISOString(),
      sale_type: 'takeaway',
      cashier_id: shift.cashier_employee_id,
      master_id: null,
      party_id: firm.id,
      contact_id: null,
      vehicle_id: null,
      comment: '',
      lines: [{ kind: 'piece', gift: false, product_id: p.id, service_id: null, qty: 1, unit_price_tyiyn: 100_000 }],
      payments: [{ method: 'debt', amount_tyiyn: 100_000 }],
      debt_signature_png: SIGNATURE_PNG,
    })

    await page.goto('/settings/cashier')
    const block = page.getByTestId('debt-backfill')
    await expect(block).toContainText('Ждут погашения долга')
    await expect(block.getByText(`ОсОО Долги ${RUN}`)).toBeAttached()
    const button = block.getByRole('button', { name: 'Начислить процент по ждущим долгам' })
    await expect(button).toBeDisabled()
    for (const width of [390, 1440]) {
      if (!process.env.SHOT_DIR) break
      await page.setViewportSize({ width, height: 900 })
      await block.screenshot({ path: `${process.env.SHOT_DIR}/debt-backfill-${width}.png` })
    }

    await api(request, 'PUT', '/settings/cashier', { op_id: crypto.randomUUID(), debt_percent_now: true })
    await page.reload()
    page.once('dialog', (d) => void d.accept())
    await page.getByTestId('debt-backfill').getByRole('button', { name: 'Начислить процент по ждущим долгам' }).click()
    await expect(page.getByText(/Начислено .* по \d+ чекам/)).toBeVisible()
    await expect(page.getByTestId('debt-backfill')).toHaveCount(0)
  } finally {
    await api(request, 'PUT', '/settings/cashier', { op_id: crypto.randomUUID(), debt_percent_now: false })
    await api(request, 'PUT', '/settings/legal', { op_id: crypto.randomUUID(), approval_mode: legal.approval_mode, approval_timeout_min: legal.approval_timeout_min })
  }
})
