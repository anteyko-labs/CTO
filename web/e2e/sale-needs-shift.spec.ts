// Продажа только при открытой смене (задача 005 Н4, ADR-169): при связи без смены касса не даёт
// пробить чек — вместо «Провести чек» «Открыть смену», плашка «пробить чек нельзя»; смену открыли —
// чек проходит. Переключатель — «Настройки → Касса», по умолчанию включён.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ownerSafe } from './safe'
import { closeShiftIfOpen } from './cashier'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-0000000005f5' }
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

test('без смены чек не пробить: «Открыть смену» на кассе, потом чек проходит', async ({ page }) => {
  await login(page)
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Смена кассы ${RUN}`, kind: 'other' })
  const item = `Щётка смены ${RUN}`
  const p = await apiPost<{ id: string }>(page.request, '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name: item, sale_price_tyiyn: 50_000 })
  await apiPost(page.request, '/receipts', {
    op_id: crypto.randomUUID(),
    paid_from: await ownerSafe(page.request),
    lines: [{ product_id: p.id, qty: 3, cost_tyiyn: 60_000 }],
  })
  const cashier = `Кассир смены ${RUN}`
  await apiPost(page.request, '/employees', { full_name: cashier, is_cashier: true, pin: '1234' })

  // Переключатель в «Настройки → Касса» включён.
  await page.goto('/settings/cashier')
  await expect(page.locator('#sale-requires-shift')).toBeChecked()

  await closeShiftIfOpen(page.request)
  await page.goto('/')
  const banner = page.getByTestId('shift-needed')
  await expect(banner).toContainText('Смена не открыта — пробить чек нельзя')
  await page.getByPlaceholder(/Сканируйте штрихкод/).fill(item)
  await page.getByRole('button', { name: new RegExp(`^${item}`) }).first().click()
  await expect(page.getByRole('listitem').filter({ hasText: item })).toBeVisible()
  // «Провести чек» нет — на его месте «Открыть смену».
  const submit = page.getByTestId('sale-submit')
  await expect(submit).toHaveText('Открыть смену')
  await expect(page.getByRole('button', { name: /Провести чек/ })).toHaveCount(0)

  await submit.click()
  const dialog = page.getByRole('dialog', { name: 'Открыть смену' })
  await dialog.locator('#shift-cashier').selectOption({ label: cashier })
  await dialog.locator('#shift-pin').fill('1234')
  await dialog.getByRole('button', { name: 'Открыть', exact: true }).click()
  await expect(dialog).toHaveCount(0)
  await expect(banner).toHaveCount(0)
  await expect(page.getByTestId('shift-cashier')).toContainText(cashier)

  // Чек в той же корзине — проходит.
  await expect(submit).toHaveText('Провести чек')
  await submit.click()
  await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()
})
