// Этап 2 в браузере: закрытие смены с недостачей и сдача кассы в сейф (SPEC-05).
import { expect, test, type APIRequestContext } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2e0' }
const RUN = Date.now().toString().slice(-8)

interface ShiftRow {
  id: string
  business_date: string
  counted_tyiyn: number | null
}

async function api<T>(request: APIRequestContext, method: 'GET' | 'POST', path: string, data?: unknown): Promise<{ status: number; body: T }> {
  const res = method === 'GET' ? await request.get(`/api/v1${path}`) : await request.post(`/api/v1${path}`, { data, headers: DEVICE })
  return { status: res.status(), body: (await res.json()) as T }
}

/** Открытая смена: смена одна в день, поэтому при повторном прогоне владелец переоткрывает сегодняшнюю. */
async function ensureOpenShift(request: APIRequestContext, cashierId: string): Promise<void> {
  const current = await api<ShiftRow | null>(request, 'GET', '/shifts/current')
  if (current.body) return
  const opened = await api(request, 'POST', '/shifts/open', { op_id: crypto.randomUUID(), cashier_employee_id: cashierId })
  if (opened.status === 200) return
  expect(opened.status).toBe(409)
  const list = await api<ShiftRow[]>(request, 'GET', '/shifts')
  const today = list.body.find((s) => s.counted_tyiyn !== null)
  expect(today, 'сегодняшняя закрытая смена').toBeTruthy()
  const reopened = await api(request, 'POST', `/shifts/${today?.id}/reopen`, { op_id: crypto.randomUUID(), reason: 'сквозной тест' })
  expect(reopened.status).toBe(200)
}

test('закрытие смены с недостачей и сдача кассы', async ({ page }) => {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()

  const cashier = `Кассир смены ${RUN}`
  const emp = await api<{ id: string }>(page.request, 'POST', '/employees', { full_name: cashier, is_cashier: true })
  expect(emp.status).toBe(200)
  await ensureOpenShift(page.request, emp.body.id)
  // Деньги в кассе, чтобы было что пересчитывать и сдавать.
  const cashIn = await api(page.request, 'POST', '/cash/movements', {
    op_id: crypto.randomUUID(),
    kind: 'cash_in',
    amount_tyiyn: 100_000,
    comment: `размен ${RUN}`,
  })
  expect(cashIn.status).toBe(200)

  await page.goto('/shift')
  await page.getByRole('button', { name: 'Закрыть смену' }).click()
  const dialog = page.locator('.fixed.inset-0').last()
  const fact = dialog.locator('#close-counted')
  const expected = await fact.inputValue()
  // Пересчитали на 10 сом меньше: недостача видна сразу, без комментария закрыть нельзя.
  const counted = (Number(expected.replace(/\s/g, '').replace(',', '.')) - 10).toFixed(2).replace('.', ',')
  await fact.fill(counted)
  await expect(dialog.getByText(/Недостача .* будет удержана с кассира/)).toBeVisible()
  await expect(dialog.getByRole('button', { name: 'Закрыть', exact: true }).last()).toBeDisabled()
  await dialog.locator('#close-comment').fill('не хватило при пересчёте')
  await dialog.getByRole('button', { name: 'Закрыть', exact: true }).last().click()

  // Сразу после закрытия — сдача кассы: оставить 50 сом на размен, остальное в сейф.
  await expect(page.getByText(/Сдать кассу · смена №/)).toBeVisible()
  const toSafe = (Number(counted.replace(/\s/g, '').replace(',', '.')) - 50).toFixed(2).replace('.', ',')
  await page.getByLabel(/В сейф/).fill(toSafe)
  await expect(page.getByText(/Останется в кассе: 50\sс/)).toBeVisible()
  await page.getByRole('button', { name: 'Перевести в сейф' }).click()
  await expect(page.getByText('Выручка в сейфе')).toBeVisible()
  await expect(page.getByText(/касса не сдана/)).toHaveCount(0)
})
