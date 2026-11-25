// Долг юрлица по видам (задача 051, ADR-215): в карточке фирмы под долгом — за машины фирмы и за
// личные машины работников; в акте — пометка у чека на личной машине и подытоги по видам.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { SIGNATURE_PNG } from './sign'
import { ownerSafe } from './safe'
import { ensureShift } from './cashier'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e051' }
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

test('карточка фирмы и акт сверки — долг двумя видами', async ({ page }) => {
  await login(page)
  const request = page.request
  const legal = await api<{ approval_mode: string; approval_timeout_min: number }>(request, 'GET', '/settings/legal')
  await api(request, 'PUT', '/settings/legal', { op_id: crypto.randomUUID(), approval_mode: 'off', approval_timeout_min: legal.approval_timeout_min })
  try {
    const shift = await ensureShift(request)
    const cat = await api<{ id: string }>(request, 'POST', '/categories', { name: `Виды ${RUN}`, kind: 'other' })
    const p = await api<{ id: string }>(request, 'POST', '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name: `Виды ${RUN} фильтр`, article: '', sale_price_tyiyn: 100_000, min_stock: 0 })
    await api(request, 'POST', '/receipts', { op_id: crypto.randomUUID(), paid_from: await ownerSafe(request), lines: [{ product_id: p.id, qty: 3, cost_tyiyn: 180_000 }] })
    const firm = await api<{ id: string }>(request, 'POST', '/parties', { role: 'customer', kind: 'company', name: `ОсОО Виды ${RUN}`, phone: `0700${RUN.slice(-6)}`, inn: `0${RUN}12345` })
    const own = await api<{ id: string }>(request, 'POST', `/parties/${firm.id}/vehicles`, { plate: `01KG${RUN.slice(-3)}AAA` })
    const personal = await api<{ id: string }>(request, 'POST', `/parties/${firm.id}/vehicles`, { plate: `01KG${RUN.slice(-3)}BBB`, owner_name: `Бакыт ${RUN}`, owner_phone: `0555${RUN.slice(-6)}` })
    const sale = (vehicle: string) =>
      api(request, 'POST', '/sales', {
        op_id: crypto.randomUUID(),
        client_time: new Date().toISOString(),
        sale_type: 'takeaway',
        cashier_id: shift.cashier_employee_id,
        master_id: null,
        party_id: firm.id,
        contact_id: null,
        vehicle_id: vehicle,
        comment: '',
        lines: [{ kind: 'piece', gift: false, product_id: p.id, service_id: null, qty: 1, unit_price_tyiyn: 100_000 }],
        payments: [{ method: 'debt', amount_tyiyn: 100_000 }],
        debt_signature_png: SIGNATURE_PNG,
      })
    await sale(personal.id)
    await sale(own.id)
    await sale(own.id)

    await page.goto(`/clients/${firm.id}`)
    await expect(page.getByTestId('debt-kinds')).toContainText(/машины фирмы и прочее 2\s000/)
    await expect(page.getByTestId('debt-kinds')).toContainText(/личные машины работников 1\s000/)

    await page.getByRole('button', { name: 'Акт сверки' }).click()
    const act = page.getByTestId('reconciliation')
    await expect(act.getByTestId('act-row')).toHaveCount(3)
    await expect(act.getByTestId('act-row').first()).toContainText(`личная — Бакыт ${RUN}`)
    await expect(act.getByTestId('act-subtotal')).toHaveCount(2)
    await expect(act.getByTestId('act-subtotal').last()).toContainText(/личные машины работников\s*1\s000/)
  } finally {
    await api(request, 'PUT', '/settings/legal', { op_id: crypto.randomUUID(), approval_mode: legal.approval_mode, approval_timeout_min: legal.approval_timeout_min })
  }
})
