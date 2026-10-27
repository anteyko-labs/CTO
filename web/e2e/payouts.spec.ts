// Выплата по видам и аванс в «Расчёте» (SPEC-36 §11, ADR-122).
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2f6' }
const RUN = Date.now().toString().slice(-8)

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function apiGet<T>(request: APIRequestContext, path: string): Promise<T> {
  const res = await request.get(`/api/v1${path}`)
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

async function apiPost<T>(request: APIRequestContext, path: string, data: unknown): Promise<T> {
  const res = await request.post(`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

interface Remains {
  remains: { percent_tyiyn: number; bonus_tyiyn: number; salary_tyiyn: number }
  balance_tyiyn: number
}

test('выплата бонуса и аванса одной операцией; в выплатах видно, за что', async ({ page }) => {
  await login(page)
  const name = `Кассир выплат ${RUN}`
  const emp = await apiPost<{ id: string }>(page.request, '/employees', { full_name: name, is_cashier: true, pin: '1234' })
  // Выплата из кассы — в открытую смену.
  if (!(await apiGet<unknown>(page.request, '/shifts/current'))) {
    await apiPost(page.request, '/shifts/open', { op_id: crypto.randomUUID(), cashier_employee_id: emp.id, cashier_pin: '1234' })
  }
  await apiPost(page.request, '/cash/movements', { op_id: crypto.randomUUID(), kind: 'cash_in', amount_tyiyn: 500_000, comment: `размен ${RUN}` })
  await apiPost(page.request, '/payroll/accruals', { op_id: crypto.randomUUID(), employee_id: emp.id, kind: 'bonus', amount_tyiyn: 100_000, comment: 'за чистоту' })

  await page.goto('/payroll')
  const row = page.getByRole('row', { name: new RegExp(name) })
  await expect(row).toContainText(/1\s000\sс/)
  await row.getByRole('button', { name: 'Выплатить' }).click()
  const dialog = page.getByRole('dialog')
  // Бонус с остатком отмечен, аванс — нет.
  await expect(dialog.getByLabel('Выплатить: Бонус')).toBeChecked()
  await expect(dialog.getByLabel('Сумма: Бонус')).toHaveValue(/1\s?000/)
  await expect(dialog.getByLabel('Выплатить: Аванс')).not.toBeChecked()
  await expect(dialog.getByText('в счёт оклада; можно сверх заработанного')).toBeVisible()
  // Больше остатка своего вида — не выплатить.
  await dialog.getByLabel('Сумма: Бонус').fill('1500')
  await expect(dialog.getByText('Бонус: больше, чем к выдаче')).toBeVisible()
  await expect(dialog.getByRole('button', { name: 'Выплатить', exact: true })).toBeDisabled()
  await dialog.getByLabel('Сумма: Бонус').fill('1000')
  await dialog.getByLabel('Выплатить: Аванс').check()
  await dialog.getByLabel('Сумма: Аванс').fill('500')
  await expect(dialog.getByText('Итого')).toBeVisible()
  await expect(dialog).toContainText(/1\s500\sс/)
  await dialog.getByRole('button', { name: 'Выплатить', exact: true }).click()
  await expect(page.getByText('Выплачено из кассы')).toBeVisible()

  const rem = await apiGet<Remains>(page.request, `/payroll/remains?employee_id=${emp.id}`)
  expect(rem.remains.bonus_tyiyn).toBe(0)
  expect(rem.remains.salary_tyiyn).toBe(-50_000)
  // «Расчёт за день»: бонус и аванс — своими колонками.
  await expect(page.getByRole('columnheader', { name: 'Аванс' })).toBeVisible()
  await expect(page.getByRole('columnheader', { name: 'Прочее' })).toHaveCount(0)
  await expect(row).toContainText(/500\sс/)

  await row.getByRole('button', { name: 'Начисления сотруднику' }).click()
  await page.getByRole('menuitem', { name: 'Начисления и выплаты за день' }).click()
  const entries = page.getByRole('dialog')
  await expect(entries.getByText(/· бонус ·/)).toBeVisible()
  await expect(entries.getByText(/· аванс ·/)).toBeVisible()
})
