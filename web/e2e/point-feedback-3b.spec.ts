// Доработки по проверке на точке, часть 3, раздел B (SPEC-29 §11–23): PIN кассира при открытии
// смены и пересчёт по остатку, вопрос «Это замена масла?», марка и модель машины из справочника и
// пробег в милях, кнопки «Внести / Изъять / Перевести» у счёта.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2f4' }
const RUN = Date.now().toString().slice(-8)
const PICKER = /Сканируйте штрихкод/

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

async function apiGet<T>(request: APIRequestContext, path: string): Promise<T> {
  const res = await request.get(`/api/v1${path}`)
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

interface Account {
  id: string
  name: string
  kind: string
  is_default: boolean
  balance_tyiyn: number
}

/** Закрыть открытую смену без расхождения и отметить, что деньги остались в кассе. */
async function closeOpenShift(request: APIRequestContext) {
  const cur = await apiGet<{ id: string; expected_tyiyn: number } | null>(request, '/shifts/current')
  if (!cur) return
  await apiPost(request, `/shifts/${cur.id}/close`, { op_id: crypto.randomUUID(), counted_tyiyn: Math.max(cur.expected_tyiyn, 0), comment: 'сквозной тест' })
  await apiPost(request, `/shifts/${cur.id}/handover`, { op_id: crypto.randomUUID(), amount_tyiyn: 0 })
}

test('PIN кассира при открытии смены, пересчёт по остатку кассы', async ({ page }) => {
  await login(page)
  const name = `Кассир PIN ${RUN}`
  await apiPost(page.request, '/employees', { full_name: name, is_cashier: true })
  await closeOpenShift(page.request)

  // PIN задаёт владелец в карточке сотрудника; в списке до этого — «без PIN».
  await page.goto('/employees')
  const row = page.getByRole('row', { name: new RegExp(name) })
  await expect(row.getByText('без PIN')).toBeVisible()
  await row.getByText(name).click()
  const box = page.getByTestId('pin-box')
  await box.locator('#employee-pin').fill('2468')
  await box.getByRole('button', { name: 'Задать PIN' }).click()
  await expect(box.getByText('задан', { exact: true })).toBeVisible()
  await page.keyboard.press('Escape')

  const till = (await apiGet<Account[]>(page.request, '/cash/accounts')).find((a) => a.kind === 'register' && a.is_default)
  expect(till).toBeTruthy()

  await page.goto('/shift')
  await page.getByRole('button', { name: 'Открыть смену' }).click()
  const dialog = page.getByRole('dialog', { name: 'Открыть смену' })
  await dialog.locator('#shift-cashier').selectOption({ label: name })
  // «Пересчитал» — по умолчанию остаток кассы с прошлой смены (SPEC-29 §16).
  const counted = await dialog.locator('#shift-counted').inputValue()
  expect(Number(counted.replace(/\s/g, '').replace(',', '.')) * 100).toBe(Math.max(till!.balance_tyiyn, 0))
  await dialog.locator('#shift-pin').fill('1111')
  await dialog.getByRole('button', { name: 'Открыть', exact: true }).click()
  await expect(dialog.getByText('неверный PIN кассира')).toBeVisible()
  await dialog.locator('#shift-pin').fill('2468')
  await dialog.getByRole('button', { name: 'Открыть', exact: true }).click()
  await expect(page.getByText('Смена открыта')).toBeVisible()
  await expect(page.getByText(new RegExp(`кассир ${name}`))).toBeVisible()
})

test('«Это замена масла?» перед чеком на вынос с маслом', async ({ page }) => {
  await login(page)
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Моторные масла ${RUN}`, kind: 'oil' })
  const code = `46${RUN}301`
  const oil = await apiPost<{ id: string }>(page.request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Масло 5W-40 ${RUN}`,
    container_ml: 4000,
    barcodes: [code],
    sale_price_tyiyn: 250_000,
  })
  const accounts = await apiGet<Account[]>(page.request, '/cash/accounts')
  const safe = accounts.find((a) => a.kind === 'safe' && a.name === 'Сейф владельца') ?? accounts.find((a) => a.kind === 'safe')
  await apiPost(page.request, '/receipts', { op_id: crypto.randomUUID(), paid_from: safe!.id, lines: [{ product_id: oil.id, qty: 8000, cost_tyiyn: 300_000 }] })
  const cashier = await apiPost<{ id: string }>(page.request, '/employees', { full_name: `Кассир масла ${RUN}`, is_cashier: true, pin: '1234' })
  await apiPost(page.request, '/employees', { full_name: `Мастер масла ${RUN}`, is_master: true })
  const cur = await apiGet<unknown>(page.request, '/shifts/current')
  if (!cur) await apiPost(page.request, '/shifts/open', { op_id: crypto.randomUUID(), cashier_employee_id: cashier.id, cashier_pin: '1234' })

  const sell = async () => {
    await page.goto('/')
    const picker = page.getByPlaceholder(PICKER)
    await picker.fill(code)
    await picker.press('Enter')
    await expect(page.getByText(`Масло 5W-40 ${RUN}`)).toBeVisible()
    await page.getByRole('button', { name: 'Карта / QR банка', exact: true }).click()
    await page.getByRole('button', { name: 'Провести чек' }).first().click()
  }

  // «Да, замена» — чек становится заменой: нужен мастер.
  await sell()
  const question = page.getByTestId('oil-question')
  await expect(question).toBeVisible()
  await question.getByRole('button', { name: 'Да, замена' }).click()
  // Замена — сразу выбор вида замены (ADR-096); закрыли, не выбрав.
  await expect(page.getByRole('dialog', { name: 'Вид замены' })).toBeVisible()
  await page.keyboard.press('Escape')
  await expect(page.locator('#master-select')).toBeVisible()
  await expect(page.getByRole('button', { name: 'В сервис' })).toHaveClass(/bg-sky-600/)
  // Передумали: снова на вынос — второй раз для этого чека не спрашиваем.
  await page.getByRole('button', { name: 'На вынос' }).click()
  await page.getByRole('button', { name: 'Провести чек' }).first().click()
  await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()
  await expect(question).toHaveCount(0)

  // «Нет, на вынос» — чек проводится как продажа на вынос.
  await sell()
  await expect(question).toBeVisible()
  await question.getByRole('button', { name: 'Нет, на вынос' }).click()
  await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()
})

test('клиент с именем и фамилией, машина из справочника с пробегом в милях', async ({ page }) => {
  await login(page)
  await page.goto('/clients')
  await page.getByRole('button', { name: 'Добавить' }).first().click()
  const first = `Азамат${RUN}`
  await page.locator('#new-client-first').fill(first)
  await expect(page.getByRole('button', { name: 'Добавить', exact: true }).last()).toBeDisabled()
  await page.locator('#new-client-last').fill('Токтогулов')
  await page.getByRole('button', { name: 'Добавить', exact: true }).last().click()
  await expect(page.getByRole('heading', { name: `Токтогулов ${first}` })).toBeVisible()
  await expect(page.getByTestId('party-person-name')).toHaveText(`${first} Токтогулов`)

  await page.getByRole('button', { name: '+ машина' }).click()
  const form = page.getByTestId('vehicle-form')
  await form.locator('#vehicle-plate').fill(`01KG${RUN.slice(-3)}MI`)
  await form.locator('#vehicle-brand').fill('Toyota')
  // Модели марки — из справочника.
  await expect(form.locator('#car-models option[value="Camry"]')).toHaveCount(1)
  await form.locator('#vehicle-model').fill('Camry')
  await form.getByRole('button', { name: 'милях' }).click()
  await form.getByRole('button', { name: 'Добавить' }).click()
  await expect(form).toHaveCount(0)
  await expect(page.getByText('Toyota Camry')).toBeVisible()
  await expect(page.getByText('пробег в милях')).toBeVisible()
  // Интервал масла показан в милях (хранится в км, ADR-084).
  await expect(page.getByText(/каждые [\d\s]+ мил(ь|и|я)/).first()).toBeVisible()
})

test('«Внести», «Изъять», «Перевести» у счёта на экране «Счета»', async ({ page }) => {
  await login(page)
  const before = await apiGet<Account[]>(page.request, '/cash/accounts')
  const safe = before.find((a) => a.name === 'Сейф магазина')!
  const owner = before.find((a) => a.name === 'Сейф владельца')!
  await page.goto('/accounts')
  const row = page.getByTestId('account-row').filter({ hasText: 'Сейф магазина' })
  await row.getByRole('button', { name: 'Внести' }).click()
  const dialog = page.getByTestId('account-money')
  await expect(dialog.getByText(/Сейчас на счёте/)).toBeVisible()
  await dialog.locator('#account-money-sum').fill('300')
  await dialog.locator('#account-money-comment').fill(`внёс владелец ${RUN}`)
  await dialog.getByRole('button', { name: 'Внести' }).click()
  await expect(page.getByText(/Внесено 300\sс: Сейф магазина/)).toBeVisible()

  await row.getByRole('button', { name: 'Перевести' }).click()
  // Куда — с остатком каждого счёта.
  await expect(dialog.locator('#account-money-to option').filter({ hasText: /Сейф владельца — / })).toHaveCount(1)
  await dialog.locator('#account-money-to').selectOption(owner.id)
  await dialog.locator('#account-money-sum').fill('100')
  await dialog.getByRole('button', { name: 'Перевести' }).click()
  await expect(page.getByText(/Переведено 100\sс: Сейф магазина → Сейф владельца/)).toBeVisible()

  await row.getByRole('button', { name: 'Изъять' }).click()
  await dialog.locator('#account-money-sum').fill('50')
  await dialog.locator('#account-money-comment').fill(`изъял ${RUN}`)
  await dialog.getByRole('button', { name: 'Изъять' }).click()
  await expect(page.getByText(/Изъято 50\sс: Сейф магазина/)).toBeVisible()

  const after = await apiGet<Account[]>(page.request, '/cash/accounts')
  expect(after.find((a) => a.id === safe.id)!.balance_tyiyn - safe.balance_tyiyn).toBe(15_000)
  expect(after.find((a) => a.id === owner.id)!.balance_tyiyn - owner.balance_tyiyn).toBe(10_000)
})
