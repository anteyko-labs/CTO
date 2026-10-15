// Экраны решений заказчика ADR-065: комиссия банка, статьи расходов, правила оплаты и отмена начислений.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2e5' }
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

test('комиссия банка и статьи расходов в настройках', async ({ page }) => {
  await login(page)
  await page.goto('/settings')
  await page.getByRole('link', { name: /Комиссия банка/ }).click()
  await expect(page.getByRole('heading', { name: 'Действует сегодня' })).toBeVisible()
  await expect(page.getByText('Карта', { exact: true }).first()).toBeVisible()
  await expect(page.getByRole('heading', { name: 'История ставок' })).toBeVisible()

  const name = `Реклама ${RUN}`
  await page.goto('/settings/expense-articles')
  await page.getByRole('button', { name: 'Добавить статью' }).click()
  await page.getByLabel('Название').fill(name)
  await page.getByRole('button', { name: 'Добавить', exact: true }).click()
  const row = page.getByRole('row', { name: new RegExp(name) })
  await expect(row).toContainText('в выборе')
  await row.getByRole('button', { name: 'Ещё действия' }).click()
  await page.getByRole('menuitem', { name: 'Скрыть' }).click()
  await expect(row).toContainText('скрыта')
})

test('правила оплаты в карточке сотрудника и прощение недостачи', async ({ page }) => {
  await login(page)
  const name = `Кассир ${RUN}`
  const employee = await apiPost<{ id: string }>(page.request, '/employees', { full_name: name, is_cashier: true, is_master: false })
  // Удержание на 50 с — его владелец отменит из расчёта за день.
  await apiPost(page.request, '/payroll/accruals', {
    op_id: crypto.randomUUID(),
    employee_id: employee.id,
    kind: 'penalty',
    amount_tyiyn: 5000,
    comment: 'сквозной тест',
  })

  await page.goto('/employees')
  await page.getByRole('row', { name: new RegExp(name) }).getByText(name).click()
  await expect(page.getByRole('heading', { name: 'Оплата' })).toBeVisible()
  // Кассиру по умолчанию — 2 % и оклад (SPEC-07).
  await expect(page.getByText(/% с валовой прибыли чеков: 2,00/)).toBeVisible()
  await page.getByRole('button', { name: 'Новое правило' }).click()
  await page.getByLabel('Вид').selectOption('per_shift')
  await page.getByLabel('Сумма, с').fill('500')
  await page.getByRole('button', { name: 'Сохранить' }).last().click()
  await expect(page.getByText(/За смену: 500/)).toBeVisible()

  await page.goto('/payroll')
  const row = page.getByRole('row', { name: new RegExp(name) })
  await row.getByRole('button', { name: 'Начисления сотруднику' }).click()
  await page.getByRole('menuitem', { name: 'Начисления и выплаты за день' }).click()
  await expect(page.getByRole('dialog', { name: new RegExp(`${name}: начисления и выплаты`) })).toBeVisible()
  await page.getByRole('button', { name: 'Отменить', exact: true }).first().click()
  await page.getByLabel('Причина').fill('ошиблись')
  await page.getByRole('button', { name: 'Отменить', exact: true }).last().click()
  await expect(page.getByText('отменено').first()).toBeVisible()
})

test('телефон сотрудника для кабинета в боте задаёт владелец', async ({ page }) => {
  await login(page)
  const name = `Мастер ${RUN}`
  await apiPost(page.request, '/employees', { full_name: name, is_master: true })
  await page.goto('/employees')
  await page.getByRole('row', { name: new RegExp(name) }).getByText(name).click()
  const phone = page.getByLabel('Телефон для бота')
  await phone.fill('12-34')
  await expect(page.getByText('Не меньше 9 цифр')).toBeVisible()
  await expect(page.getByRole('button', { name: 'Сохранить' }).first()).toBeDisabled()
  await phone.fill(`+996 700 ${RUN.slice(0, 3)} ${RUN.slice(3, 6)}`)
  await page.getByRole('button', { name: 'Сохранить' }).first().click()
  await expect(page.getByRole('dialog')).toBeHidden()
  await page.getByRole('row', { name: new RegExp(name) }).getByText(name).click()
  await expect(page.getByLabel('Телефон для бота')).toHaveValue(`+996 700 ${RUN.slice(0, 3)} ${RUN.slice(3, 6)}`)
})
