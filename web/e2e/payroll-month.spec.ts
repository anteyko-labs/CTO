// Действия в «Расчёте за месяц» (задача 002, П13; SPEC-07): выплата, аванс, бонус и «Исправить
// последнюю выплату» прямо из строки работника; после каждого действия строка пересчитывается сразу.
// Администратору — только кнопки по его правам.
import { expect, test, type APIRequestContext, type Locator, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e013' }
const RUN = Date.now().toString().slice(-8)

async function login(page: Page, user: string, password: string) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(user)
  await page.getByLabel('Пароль').fill(password)
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

/** Ячейка строки месяца по колонке: «Бонус», «Выплачено», «Аванс», «На конец». */
const COLS = ['Сотрудник', 'На начало', 'Оклад', '% с прибыли', 'Мастеру', 'За смену', 'Бонус', 'Удержано', 'Начислено', 'Выплачено', 'Аванс', 'На конец']
const cell = (row: Locator, col: string) => row.locator('td').nth(COLS.indexOf(col))
/** Сумма как на экране: «1 200 с» с неразрывными пробелами. */
const som = (text: string) => new RegExp(`^${text.replace(/ /g, '\\s')}\\sс$`)

test('выплата, аванс, бонус и «Исправить» из строки месяца; итоги строки пересчитываются', async ({ page }) => {
  await login(page, OWNER, OWNER_PASSWORD)
  const name = `Мастер месяца ${RUN}`
  const emp = await apiPost<{ id: string }>(page.request, '/employees', { full_name: name, is_master: true })
  // Выплата из кассы — в открытую смену.
  if (!(await apiGet<unknown>(page.request, '/shifts/current'))) {
    const cashier = await apiPost<{ id: string }>(page.request, '/employees', { full_name: `Кассир месяца ${RUN}`, is_cashier: true, pin: '1234' })
    await apiPost(page.request, '/shifts/open', { op_id: crypto.randomUUID(), cashier_employee_id: cashier.id, cashier_pin: '1234' })
  }
  await apiPost(page.request, '/cash/movements', { op_id: crypto.randomUUID(), kind: 'cash_in', amount_tyiyn: 500_000, comment: `размен ${RUN}` })
  await apiPost(page.request, '/payroll/accruals', { op_id: crypto.randomUUID(), employee_id: emp.id, kind: 'bonus', amount_tyiyn: 100_000, comment: 'за чистоту' })

  await page.goto('/payroll')
  await page.getByRole('button', { name: 'За месяц' }).click()
  const row = page.getByRole('row', { name: new RegExp(name) })
  await expect(cell(row, 'Бонус')).toHaveText(som('1 000'))
  await expect(cell(row, 'На конец')).toHaveText(som('1 000'))

  // «Выплатить» — то же окно по видам, что в «За день».
  await row.getByRole('button', { name: 'Выплатить' }).click()
  let dialog = page.getByRole('dialog', { name: `Выплата: ${name}` })
  await expect(dialog.getByLabel('Выплатить: Бонус')).toBeChecked()
  await dialog.getByLabel('Сумма: Бонус').fill('600')
  await dialog.getByRole('button', { name: 'Выплатить', exact: true }).click()
  await expect(page.getByText('Выплачено из кассы')).toBeVisible()
  await expect(dialog).toHaveCount(0)
  await expect(cell(row, 'Выплачено')).toHaveText(som('600'))
  await expect(cell(row, 'На конец')).toHaveText(som('400'))

  // «Аванс» — в окне только строка аванса, сразу отмечена.
  await row.getByRole('button', { name: 'Аванс' }).click()
  dialog = page.getByRole('dialog', { name: `Аванс: ${name}` })
  await expect(dialog.getByLabel('Выплатить: Аванс')).toBeChecked()
  await expect(dialog.getByLabel('Выплатить: Бонус')).toHaveCount(0)
  await dialog.getByLabel('Сумма: Аванс').fill('300')
  await dialog.getByRole('button', { name: 'Выдать аванс' }).click()
  await expect(dialog).toHaveCount(0)
  await expect(cell(row, 'Аванс')).toHaveText(som('300'))
  await expect(cell(row, 'На конец')).toHaveText(som('100'))

  // «Бонус» владельца — сегодняшним днём (текущий месяц).
  await row.getByRole('button', { name: `Начисления и выплаты: ${name}` }).click()
  await page.getByRole('menuitem', { name: 'Бонус' }).click()
  dialog = page.getByRole('dialog', { name: new RegExp(`^Бонус за .*: ${name}$`) })
  await dialog.getByLabel('Сумма, с').fill('200')
  await dialog.getByLabel('Причина').fill('за порядок')
  await dialog.getByRole('button', { name: 'Записать' }).click()
  await expect(page.getByText('Бонус начислен')).toBeVisible()
  await expect(dialog).toHaveCount(0)
  await expect(cell(row, 'Бонус')).toHaveText(som('1 200'))
  await expect(cell(row, 'На конец')).toHaveText(som('300'))

  // «Исправить последнюю выплату» — последняя здесь аванс 300: сторно и новая на 250 одной операцией.
  await row.getByRole('button', { name: `Начисления и выплаты: ${name}` }).click()
  await page.getByRole('menuitem', { name: 'Исправить последнюю выплату' }).click()
  dialog = page.getByRole('dialog', { name: new RegExp(`^Исправить выплату № \\d+: ${name}$`) })
  await expect(dialog.getByText(/Аванс, деньги уйдут/)).toBeVisible()
  await dialog.getByLabel('Сумма, с').fill('250')
  await dialog.locator('#correction-comment').fill('выдали меньше')
  await dialog.getByRole('button', { name: 'Сохранить исправление' }).click()
  await expect(page.getByText(/Исправлено: /)).toBeVisible()
  await expect(dialog).toHaveCount(0)
  await expect(cell(row, 'Аванс')).toHaveText(som('250'))
  await expect(cell(row, 'Выплачено')).toHaveText(som('600'))
  await expect(cell(row, 'На конец')).toHaveText(som('350'))

  // Исходная выплата не изменена: отменена сторно, рядом — новая (I4).
  const today = new Date().toLocaleDateString('sv-SE', { timeZone: 'Asia/Bishkek' })
  const entries = await apiGet<{ payouts: { amount_tyiyn: number; reversal_of: string | null; reversed: boolean }[] }>(
    page.request,
    `/payroll/entries?employee_id=${emp.id}&from=${today.slice(0, 8)}01&to=${today}`,
  )
  const amounts = entries.payouts.map((p) => [p.amount_tyiyn, p.reversed, Boolean(p.reversal_of)])
  expect(amounts).toEqual(expect.arrayContaining([[30_000, true, false], [-30_000, false, true], [25_000, false, false], [60_000, false, false]]))
})

test('администратор в месяце: «Выплатить» по праву, без аванса, бонуса и исправления', async ({ page, browser }) => {
  await login(page, OWNER, OWNER_PASSWORD)
  const name = `Мастер админа ${RUN}`
  const emp = await apiPost<{ id: string }>(page.request, '/employees', { full_name: name, is_master: true })
  await apiPost(page.request, '/payroll/accruals', { op_id: crypto.randomUUID(), employee_id: emp.id, kind: 'bonus', amount_tyiyn: 10_000, comment: 'за неделю' })
  const admin = `paymonth${RUN}`
  const password = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  await apiPost(page.request, '/users', { login: admin, password, full_name: `Админ месяца ${RUN}`, role: 'admin' })

  const ctx = await browser.newContext()
  const a = await ctx.newPage()
  await login(a, admin, password)
  await a.goto('/payroll')
  await a.getByRole('button', { name: 'За месяц' }).click()
  const row = a.getByRole('row', { name: new RegExp(name) })
  await expect(row).toBeVisible()
  // Права по умолчанию: выплаты мастерам есть, «Аванс мастерам» — нет; начисления — владелец.
  await expect(row.getByRole('button', { name: 'Выплатить' })).toBeVisible()
  await expect(row.getByRole('button', { name: 'Аванс' })).toHaveCount(0)
  await expect(row.getByRole('button', { name: `Начисления и выплаты: ${name}` })).toHaveCount(0)
  await expect(row.getByRole('button', { name: 'Подробно' })).toHaveCount(0)
  // Скрытая кнопка — не защита (I12).
  const denied = await a.request.post('/api/v1/payroll/accruals', {
    data: { op_id: crypto.randomUUID(), employee_id: emp.id, kind: 'bonus', amount_tyiyn: 1_000, comment: 'сам себе' },
    headers: DEVICE,
  })
  expect(denied.status()).toBe(403)
  await ctx.close()
})
