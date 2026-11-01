// Задача 005 Н1 (ADR-{n1}): личный вход сотрудника. Владелец заводит вход в карточке сотрудника →
// кассир входит своим логином → открывает смену на себя без PIN → пробивает чек → видит «Мой
// заработок» и меняет свой пароль в «Профиле».
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e501' }
const RUN = Date.now().toString().slice(-8)
const PICKER = /Сканируйте штрихкод/

test.use({ viewport: { width: 1366, height: 768 } })

async function login(page: Page, user: string, password: string) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(user)
  await page.getByLabel('Пароль').fill(password)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'POST' | 'PUT' | 'PATCH', path: string, data: unknown): Promise<T> {
  const res = await request.fetch(`/api/v1${path}`, { method, data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

/** Закрыть открытую смену, если она есть: пересчитано ровно столько, сколько должно быть. */
async function closeOpenShift(request: APIRequestContext) {
  const cur = (await (await request.get('/api/v1/shifts/current')).json()) as { id: string; expected_tyiyn: number } | null
  if (!cur) return
  await api(request, 'POST', `/shifts/${cur.id}/close`, { op_id: crypto.randomUUID(), counted_tyiyn: cur.expected_tyiyn, comment: 'e2e' })
  await api(request, 'POST', `/shifts/${cur.id}/handover`, { op_id: crypto.randomUUID(), amount_tyiyn: cur.expected_tyiyn })
}

test('кассир со своим входом: смена на себя без PIN, чек, «Мой заработок», свой пароль', async ({ page, browser }) => {
  await login(page, OWNER, OWNER_PASSWORD)
  const cat = await api<{ id: string }>(page.request, 'POST', '/categories', { name: `Личный вход ${RUN}`, kind: 'other' })
  const item = `Ароматизатор ${RUN}`
  const p = await api<{ id: string }>(page.request, 'POST', '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name: item, sale_price_tyiyn: 50_000 })
  await api(page.request, 'POST', '/receipts', {
    op_id: crypto.randomUUID(),
    paid_from: await ownerSafe(page.request),
    lines: [{ product_id: p.id, qty: 5, cost_tyiyn: 100_000 }],
  })
  const name = `Айбек ${RUN}`
  await api(page.request, 'POST', '/employees', { full_name: name, is_cashier: true, pin: '7531' })
  await closeOpenShift(page.request)

  // Владелец: «Сотрудники» → карточка → «Вход в программу».
  const user = `aibek${RUN}`
  const password = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  await page.goto('/employees')
  await page.getByRole('row', { name: new RegExp(name) }).click()
  const box = page.getByTestId('login-box')
  await expect(box.getByText('нет', { exact: true })).toBeVisible()
  await box.locator('#employee-login').fill(user)
  await box.locator('#employee-password').fill('short')
  await expect(box.getByRole('button', { name: 'Завести вход' })).toBeDisabled()
  await box.locator('#employee-password').fill(password)
  await box.getByRole('button', { name: 'Завести вход' }).click()
  await expect(page.getByText('Вход заведён')).toBeVisible()
  await expect(box.getByText('включён', { exact: true })).toBeVisible()
  await page.keyboard.press('Escape')
  await expect(page.getByRole('row', { name: new RegExp(name) }).getByText(`вход: ${user}`)).toBeVisible()
  // «Пользователи и права»: у входа виден сотрудник.
  await page.goto('/users')
  await expect(page.getByRole('row', { name: new RegExp(user) }).getByTestId('user-employee')).toHaveText(`сотрудник: ${name}`)

  const ctx = await browser.newContext({ viewport: { width: 1366, height: 768 } })
  const a = await ctx.newPage()
  try {
    await login(a, user, password)
    await expect(a.getByRole('link', { name: 'Мой заработок' })).toBeVisible()

    // Смена — на себя, без PIN.
    await a.goto('/shift')
    await a.getByRole('button', { name: 'Открыть смену' }).click()
    const dialog = a.getByRole('dialog', { name: 'Открыть смену' })
    await expect(dialog.getByTestId('shift-self')).toBeVisible()
    await expect(dialog.locator('#shift-pin')).toHaveCount(0)
    await dialog.getByRole('button', { name: 'Открыть', exact: true }).click()
    await expect(a.getByText('Смена открыта')).toBeVisible()
    await expect(a.getByText(new RegExp(`кассир ${name}`))).toBeVisible()

    // Чек.
    await a.goto('/')
    await a.getByPlaceholder(PICKER).fill(item)
    await a.getByRole('button', { name: new RegExp(`^${item}`) }).first().click()
    await expect(a.getByTestId('cart')).toContainText(item)
    await a.getByRole('button', { name: 'Провести чек' }).first().click()
    await expect(a.getByText(/Чек № \d+ проведён/)).toBeVisible()

    // «Мой заработок» — только своё.
    await a.getByRole('link', { name: 'Мой заработок' }).click()
    await expect(a.getByRole('heading', { name: 'Мой заработок' })).toBeVisible()
    await expect(a.getByTestId('my-earnings-accrued')).toBeVisible()
    await expect(a.getByTestId('my-earnings-owed')).toBeVisible()
    const mine = (await (await a.request.get('/api/v1/me/earnings')).json()) as { full_name: string }
    expect(mine.full_name).toBe(name)

    // «Профиль» → «Сменить свой пароль».
    const next = `${password}-2`
    await a.goto('/profile')
    const pw = a.getByTestId('profile-password')
    await pw.locator('#profile-old').fill(password)
    await pw.locator('#profile-new').fill(next)
    await pw.locator('#profile-again').fill(next)
    await pw.getByRole('button', { name: 'Сменить пароль' }).click()
    await expect(a.getByText(/Пароль изменён/)).toBeVisible()
    const relog = await browser.newContext()
    const b = await relog.newPage()
    await login(b, user, next)
    await relog.close()
  } finally {
    await ctx.close()
    await closeOpenShift(page.request)
  }
})
