// Оклад (задача 003 Н11): вкладка «Оклад» — ставка видна до начисления, аванс уменьшает «к выплате»,
// «Начислить оклад» за период работника (день расчёта 20 → «20.10–19.11»); в окне «Выплата» — строка
// оклада; владелец платит в прошлом месяце «датой выплаты» (деньги уходят сегодня); «Настройки → Зарплата».
import { expect, test, type APIRequestContext, type Locator, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e031' }
const RUN = Date.now().toString().slice(-8)

async function login(page: Page, user: string, password: string) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(user)
  await page.getByLabel('Пароль').fill(password)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'GET' | 'POST' | 'PUT', path: string, data?: unknown): Promise<T> {
  const res = await request.fetch(`/api/v1${path}`, { method, data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

/** Есть открытая смена — в кассе размен, чтобы выплата из кассы прошла; нет — владелец платит из сейфа. */
async function ensureCash(request: APIRequestContext) {
  const shift = await api<unknown>(request, 'GET', '/shifts/current')
  if (shift) await api(request, 'POST', '/cash/movements', { op_id: crypto.randomUUID(), kind: 'cash_in', amount_tyiyn: 1_000_000, comment: `размен ${RUN}` })
}

const today = () => new Date().toLocaleDateString('sv-SE', { timeZone: 'Asia/Bishkek' })
const pad = (n: number) => String(n).padStart(2, '0')
/** Период дня 20, в который входит сегодня: те же правила, что `periodOf` (проверены юнит-тестом). */
function period20(d: string): { from: string; to: string } {
  const [y, m, dd] = d.split('-').map(Number)
  const start = dd >= 20 ? new Date(Date.UTC(y, m - 1, 20)) : new Date(Date.UTC(y, m - 2, 20))
  const end = new Date(Date.UTC(start.getUTCFullYear(), start.getUTCMonth() + 1, 19))
  const s = (t: Date) => `${t.getUTCFullYear()}-${pad(t.getUTCMonth() + 1)}-${pad(t.getUTCDate())}`
  return { from: s(start), to: s(end) }
}
const label = (from: string, to: string) => `${from.slice(8)}.${from.slice(5, 7)}–${to.slice(8)}.${to.slice(5, 7)}`
/** Сумма как на экране: «1 200 с» с неразрывными пробелами. */
const som = (text: string) => new RegExp(`${text.replace(/ /g, '\\s')}\\sс`)

const COLS = ['Сотрудник', 'Период', 'Ставка', 'Начислено', 'Авансом', 'Выдано окладом', 'С прошлых', 'К выплате']
const cell = (row: Locator, col: string) => row.locator('td').nth(COLS.indexOf(col))

test('вкладка «Оклад»: ставка до начисления, аванс, начисление за период дня расчёта', async ({ page }) => {
  await login(page, OWNER, OWNER_PASSWORD)
  const name = `Кассир оклада ${RUN}`
  await ensureCash(page.request)
  const emp = await api<{ id: string }>(page.request, 'POST', '/employees', { full_name: name, is_cashier: true })
  // День расчёта — в карточке сотрудника, раздел «Оплата».
  await api(page.request, 'PUT', `/employees/${emp.id}/salary-day`, { salary_day: 20 })
  const p = period20(today())

  await page.goto('/payroll')
  await page.getByRole('button', { name: 'Оклад', exact: true }).click()
  const row = page.getByTestId('payroll-salary').getByRole('row', { name: new RegExp(name) })
  await expect(cell(row, 'Период')).toContainText(label(p.from, p.to))
  await expect(cell(row, 'Период')).toContainText('день расчёта 20-е')
  await expect(cell(row, 'Ставка')).toHaveText(som('30 000'))
  await expect(cell(row, 'Начислено')).toHaveText('не начислен')
  await expect(cell(row, 'К выплате')).toContainText(som('30 000'))

  // Аванс из вкладки: в окне «Выплата» — строка оклада.
  await row.getByRole('button', { name: 'Выплатить' }).click()
  const dialog = page.getByRole('dialog', { name: `Выплата: ${name}` })
  await expect(dialog.getByTestId('payout-salary')).toContainText(`за ${label(p.from, p.to)}, не начислен`)
  await dialog.getByLabel('Выплатить: Аванс').check()
  await dialog.getByLabel('Сумма: Аванс').fill('5000')
  await dialog.getByRole('button', { name: 'Выплатить', exact: true }).click()
  await expect(dialog).toHaveCount(0)
  await expect(cell(row, 'Авансом')).toHaveText(som('5 000'))
  await expect(cell(row, 'К выплате')).toContainText(som('25 000'))

  // «Начислить оклад» — за период работника.
  await row.getByRole('button', { name: 'Начислить оклад' }).click()
  const sal = page.getByRole('dialog', { name: `Оклад за ${label(p.from, p.to)}: ${name}` })
  await expect(sal.getByLabel('Сумма, с')).toHaveValue('30000')
  await sal.getByRole('button', { name: 'Начислить' }).click()
  await expect(page.getByText('Оклад начислен')).toBeVisible()
  await expect(cell(row, 'Начислено')).toHaveText(som('30 000'))
  await expect(cell(row, 'К выплате')).toContainText(som('25 000'))
  await expect(row.getByRole('button', { name: 'Начислить оклад' })).toHaveCount(0)
})

test('владелец платит в прошлом месяце датой выплаты; деньги уходят сегодня', async ({ page }) => {
  await login(page, OWNER, OWNER_PASSWORD)
  const name = `Мастер даты ${RUN}`
  await ensureCash(page.request)
  const emp = await api<{ id: string }>(page.request, 'POST', '/employees', { full_name: name, is_master: true })
  const t = today()
  const [y, m] = t.split('-').map(Number)
  const prevLast = new Date(Date.UTC(y, m - 1, 0))
  const last = `${prevLast.getUTCFullYear()}-${pad(prevLast.getUTCMonth() + 1)}-${pad(prevLast.getUTCDate())}`
  await api(page.request, 'POST', '/payroll/accruals', {
    op_id: crypto.randomUUID(),
    employee_id: emp.id,
    kind: 'bonus',
    amount_tyiyn: 100_000,
    business_date: last,
    comment: 'за прошлый месяц',
  })

  await page.goto('/payroll')
  await page.getByRole('button', { name: 'За месяц' }).click()
  await page.getByRole('button', { name: 'Прошлый месяц' }).click()
  const row = page.getByRole('row', { name: new RegExp(name) })
  await row.getByRole('button', { name: 'Выплатить' }).click()
  const dialog = page.getByRole('dialog', { name: `Выплата: ${name}` })
  await expect(dialog.locator('#payout-date')).toHaveValue(last)
  await expect(dialog.getByText(new RegExp(`в расчёте — ${last.slice(8)}\\.${last.slice(5, 7)}, деньги уйдут сегодня`))).toBeVisible()
  await dialog.getByLabel('Сумма: Бонус').fill('400')
  await dialog.getByRole('button', { name: 'Выплатить', exact: true }).click()
  await expect(page.getByText(new RegExp(`Выплачено за ${last.slice(8)}\\.${last.slice(5, 7)}`))).toBeVisible()
  await expect(dialog).toHaveCount(0)
  // Строка прошлого месяца пересчитана: выплачено 400, на конец 600.
  await expect(row.locator('td').nth(9)).toHaveText(som('400'))
  await expect(row.locator('td').nth(11)).toHaveText(som('600'))

  const entries = await api<{ payouts: { pay_date: string; created_at: string }[] }>(
    page.request,
    'GET',
    `/payroll/entries?employee_id=${emp.id}&from=${last.slice(0, 8)}01&to=${last}`,
  )
  expect(entries.payouts.map((p) => p.pay_date)).toEqual([last])
  expect(new Date(entries.payouts[0].created_at).toLocaleDateString('sv-SE', { timeZone: 'Asia/Bishkek' })).toBe(t)
})

test('«Настройки → Зарплата»: по умолчанию 35 дней, выплаты без смены, напоминание', async ({ page }) => {
  await login(page, OWNER, OWNER_PASSWORD)
  await page.goto('/settings')
  await page.getByRole('link', { name: /Зарплата/ }).click()
  const card = page.getByTestId('payroll-settings')
  await expect(card.locator('#payroll-backdate')).toHaveValue('35')
  await expect(card.getByLabel('Выплаты только при открытой смене')).not.toBeChecked()
  await expect(card.getByLabel('В день расчёта оклада в 09:00 — напоминание')).toBeChecked()
  await card.getByLabel('Показывать заработанное по окладу на сегодня').check()
  await card.getByRole('button', { name: 'Сохранить' }).click()
  await expect(page.getByText('Настройки зарплаты сохранены')).toBeVisible()
  const s = await api<{ salary_show_to_date: boolean }>(page.request, 'GET', '/settings/payroll')
  expect(s.salary_show_to_date).toBe(true)
  // Назад как было: другие проверки ждут настроек по умолчанию.
  await api(page.request, 'PUT', '/settings/payroll', { payout_backdate_days: 35, payout_requires_shift: false, salary_show_to_date: false, salary_due_notify: true })
})
