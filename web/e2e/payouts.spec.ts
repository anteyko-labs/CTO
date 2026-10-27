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

test('закрытие смены: кассира смены нет, кассиру — только с паролем владельца, по видам', async ({ page }) => {
  await login(page)
  // Прежнюю смену — закрыть: эту открывает свой кассир.
  const cur = await apiGet<{ id: string; expected_tyiyn: number } | null>(page.request, '/shifts/current')
  if (cur) {
    await apiPost(page.request, `/shifts/${cur.id}/close`, { op_id: crypto.randomUUID(), counted_tyiyn: Math.max(cur.expected_tyiyn, 0), comment: 'сквозной тест' })
    await apiPost(page.request, `/shifts/${cur.id}/handover`, { op_id: crypto.randomUUID(), amount_tyiyn: 0 })
  }
  const own = `Кассир смены ${RUN}`
  const other = `Кассир другой ${RUN}`
  const a = await apiPost<{ id: string }>(page.request, '/employees', { full_name: own, is_cashier: true, pin: '2468' })
  const b = await apiPost<{ id: string }>(page.request, '/employees', { full_name: other, is_cashier: true, pin: '1357' })
  const shift = await apiPost<{ id: string }>(page.request, '/shifts/open', { op_id: crypto.randomUUID(), cashier_employee_id: a.id, cashier_pin: '2468' })
  await apiPost(page.request, '/cash/movements', { op_id: crypto.randomUUID(), kind: 'cash_in', amount_tyiyn: 500_000, comment: `размен ${RUN}` })
  for (const id of [a.id, b.id]) {
    await apiPost(page.request, '/payroll/accruals', { op_id: crypto.randomUUID(), employee_id: id, kind: 'bonus', amount_tyiyn: 40_000, comment: 'за неделю' })
  }

  // Сервер: выплата кассиру при закрытии без пароля владельца — 403.
  const cur2 = await apiGet<{ expected_tyiyn: number }>(page.request, '/shifts/current')
  const denied = await page.request.post(`/api/v1/shifts/${shift.id}/close`, {
    headers: DEVICE,
    data: {
      op_id: crypto.randomUUID(),
      counted_tyiyn: cur2.expected_tyiyn - 40_000,
      payouts: [{ op_id: crypto.randomUUID(), employee_id: b.id, lines: [{ purpose: 'bonus', amount_tyiyn: 40_000 }] }],
    },
  })
  expect(denied.status()).toBe(403)
  expect(await denied.text()).toContain('Кассиру выплачивает владелец')

  await page.goto('/shift')
  await page.getByRole('button', { name: 'Закрыть смену' }).click()
  const dialog = page.getByRole('dialog', { name: 'Закрыть смену' })
  // Кассира этой смены в списке нет.
  await expect(dialog.getByTestId('close-pay-row').filter({ hasText: other })).toHaveCount(1)
  await expect(dialog.getByTestId('close-pay-row').filter({ hasText: own })).toHaveCount(0)
  const row = dialog.getByTestId('close-pay-row').filter({ hasText: other })
  await row.getByRole('checkbox').first().check()
  // Строки по видам — то же окно, что в «Расчёте».
  await expect(dialog.getByLabel(`Выплатить: Бонус (${other})`)).toBeChecked()
  await expect(dialog.getByLabel(`Сумма: Бонус (${other})`)).toHaveValue(/400/)
  await expect(dialog.getByText(/− выплаты 400\sс/)).toBeVisible()
  const submit = dialog.getByRole('button', { name: 'Выплатить отмеченным и закрыть' })
  // Без пароля владельца — не закрыть; неверный пароль — ошибка сервера.
  await expect(dialog.getByText(/пароль владельца — выплата кассиру/)).toBeVisible()
  await expect(submit).toBeDisabled()
  await dialog.locator('#close-owner-password').fill('не тот пароль')
  await submit.click()
  await expect(dialog.getByText('неверный пароль владельца')).toBeVisible()
  await dialog.locator('#close-owner-password').fill(OWNER_PASSWORD)
  await submit.click()
  await expect(page.getByText(/Выплачено 1, смена закрыта/)).toBeVisible()
  await page.getByRole('button', { name: 'Деньги остались в кассе' }).click()
  await expect(page.getByText('Отмечено: деньги остались в кассе')).toBeVisible()

  const rem = await apiGet<Remains>(page.request, `/payroll/remains?employee_id=${b.id}`)
  expect(rem.remains.bonus_tyiyn).toBe(0)
  const own_ = await apiGet<Remains>(page.request, `/payroll/remains?employee_id=${a.id}`)
  expect(own_.remains.bonus_tyiyn).toBe(40_000)
})
