// Касса по доработкам 21.10.2026 (SPEC-30 §1–4): оплата «По счёту», вид замены в чеке «В сервис» с
// ценой по нашему маслу, данные клиента для замены, правка цены только с правом «Скидка в чеке».
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { pickCashier } from './cashier'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e304' }
const RUN = Date.now().toString().slice(-8)
const PICKER = /Сканируйте штрихкод/

async function login(page: Page, user = OWNER, password = OWNER_PASSWORD) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(user)
  await page.getByLabel('Пароль').fill(password)
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

/** Товар со штрихкодом и остатком. */
async function goods(request: APIRequestContext, kind: string, name: string, code: string, price: number, extra: Record<string, unknown> = {}) {
  const cat = await apiPost<{ id: string }>(request, '/categories', { name: `Кат ${name}`, kind })
  const p = await apiPost<{ id: string }>(request, '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name, barcodes: [code], sale_price_tyiyn: price, ...extra })
  const qty = extra.container_ml ? 8000 : 5
  await apiPost(request, '/receipts', { op_id: crypto.randomUUID(), paid_from: await ownerSafe(request), lines: [{ product_id: p.id, qty, cost_tyiyn: Math.floor(price / 2) * (extra.container_ml ? 2 : 5) }] })
  return p
}

async function scan(page: Page, code: string) {
  const picker = page.getByPlaceholder(PICKER)
  await picker.fill(code)
  await picker.press('Enter')
}

const payButton = (page: Page, label: string) => page.locator('[data-pay]').getByRole('button', { name: label, exact: true })

interface Sale {
  id: string
  master_fee_tyiyn: number
  total_tyiyn: number
  lines: { kind: string; unit_price_tyiyn: number; list_price_tyiyn: number }[]
  payments: { method: string; account_id?: string | null }[]
}

test('«По счёту»: без клиента не провести, с фирмой — проведено без комиссии', async ({ page }) => {
  await login(page)
  const cashier = `Кассир по счёту ${RUN}`
  await apiPost(page.request, '/employees', { full_name: cashier, is_cashier: true, pin: '1234' })
  const code = `30${RUN}101`
  const product = `Фильтр по счёту ${RUN}`
  await goods(page.request, 'filter', product, code, 80_000)
  const firm = `ОсОО Перечисление ${RUN}`
  await apiPost(page.request, '/parties', { name: firm, kind: 'company', inn: `0120${RUN}55` })

  await page.goto('/')
  await scan(page, code)
  await expect(page.getByRole('listitem').filter({ hasText: product })).toBeVisible()
  await pickCashier(page, cashier)
  await payButton(page, 'По счёту').click()
  await expect(page.getByTestId('pay-dest')).toHaveText(/^→ .+ · без комиссии$/)
  await expect(page.getByTestId('invoice-needs-client')).toBeVisible()
  const submit = page.getByRole('button', { name: 'Провести чек' })
  await expect(submit).toBeDisabled()
  await page.locator('[data-client-input]').fill(firm)
  await page.getByRole('button', { name: new RegExp(`^${firm}`) }).click()
  await expect(page.getByTestId('invoice-needs-client')).toHaveCount(0)
  await submit.click()
  await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()
  const href = await page.getByRole('link', { name: 'Открыть', exact: true }).getAttribute('href')
  const sale = await apiGet<Sale>(page.request, href ?? '')
  expect(sale.payments.map((p) => p.method)).toEqual(['invoice'])
  await page.goto(href ?? '/')
  await expect(page.getByText('По счёту').first()).toBeVisible()
})

test('«В сервис»: вид замены, бесплатно с нашим маслом, данные клиента', async ({ page }) => {
  await login(page)
  const cashier = `Кассир замены ${RUN}`
  const master = `Мастер замены ${RUN}`
  await apiPost(page.request, '/employees', { full_name: cashier, is_cashier: true, pin: '1234' })
  await apiPost(page.request, '/employees', { full_name: master, is_master: true })
  const code = `30${RUN}202`
  const oil = `Масло замены ${RUN}`
  await goods(page.request, 'oil', oil, code, 250_000, { container_ml: 4000 })
  const kind = `Замена в моторе ${RUN}`
  await apiPost(page.request, '/services', { name: kind, price_tyiyn: 30_000, master_fee_tyiyn: 3_500, free_with_oil: true })
  const client = `Заменов Клиент${RUN}`
  const party = await apiPost<{ id: string }>(page.request, '/parties', { name: client, kind: 'person' })
  const plate = `01KG${RUN.slice(-3)}KND`
  await apiPost(page.request, `/parties/${party.id}/vehicles`, { plate })

  await page.goto('/')
  await scan(page, code)
  await expect(page.getByRole('listitem').filter({ hasText: oil })).toBeVisible()
  await pickCashier(page, cashier)
  // «В сервис» сразу открывает выбор вида замены: цена клиенту или «бесплатно — масло наше», ставка мастеру.
  await page.getByRole('button', { name: 'В сервис' }).click()
  const dialog = page.getByRole('dialog', { name: 'Вид замены' })
  await expect(dialog).toBeVisible()
  await dialog.getByPlaceholder('Найти услугу').fill(kind)
  const choice = dialog.getByTestId('service-choices').getByRole('button', { name: new RegExp(kind) })
  await expect(choice).toContainText('бесплатно — масло наше')
  await expect(choice).toContainText(/мастеру 35\sс/)
  // Отметка, а не выбор сразу: окно закрывает «Готово» (видов можно отметить несколько).
  await choice.click()
  await expect(choice).toHaveAttribute('aria-pressed', 'true')
  await dialog.getByRole('button', { name: 'Готово' }).click()
  await expect(dialog).toHaveCount(0)
  const kindLine = page.getByRole('listitem').filter({ hasText: kind })
  await expect(kindLine.getByLabel('Цена', { exact: true })).toHaveValue('0')
  await expect(page.getByTestId('service-kinds')).toContainText('бесплатно — масло наше')

  // Масло убрали — замена платная по цене клиенту; вернули — снова бесплатно.
  await page.getByRole('listitem').filter({ hasText: oil }).getByRole('button', { name: 'Удалить строку' }).click()
  await expect(kindLine.getByLabel('Цена', { exact: true })).toHaveValue('300')
  await scan(page, code)
  await expect(kindLine.getByLabel('Цена', { exact: true })).toHaveValue('0')

  await page.locator('#master-select').selectOption({ label: master })
  // Данные клиента обязательны: без клиента, телефона, машины и пробега — не провести.
  const submit = page.getByRole('button', { name: 'Провести чек' })
  const needs = page.getByTestId('service-client-needs')
  await expect(needs).toContainText('телефон клиента')
  await expect(submit).toBeDisabled()
  // Клиента находят по госномеру — машина встаёт сама.
  await page.locator('[data-client-input]').fill(plate)
  await page.getByRole('button', { name: new RegExp(`^${client}`) }).click()
  await expect(needs).toContainText('телефон клиента')
  await expect(needs).not.toContainText('машина с госномером')
  await page.locator('#client-phone').fill(`0700${RUN.slice(-6)}`)
  await page.getByRole('button', { name: 'Записать в карточку' }).click()
  await expect(needs).not.toContainText('телефон клиента')
  await page.locator('#mileage').fill('150000')
  await expect(needs).toHaveCount(0)
  await page.getByRole('button', { name: 'Без сдачи' }).click()
  await submit.click()
  await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()
  const href = await page.getByRole('link', { name: 'Открыть', exact: true }).getAttribute('href')
  const sale = await apiGet<Sale>(page.request, href ?? '')
  expect(sale.master_fee_tyiyn).toBe(3_500)
  const service = sale.lines.find((l) => l.kind === 'service')
  expect(service?.unit_price_tyiyn).toBe(0)
  expect(service?.list_price_tyiyn).toBe(0)

  // «Продаваемые» → «Замены»: вид замены, сколько мастерам, масло в этих чеках.
  await page.goto('/top-products')
  await page.getByRole('tab', { name: 'Замены' }).click()
  const row = page.getByTestId('service-kind-row').filter({ hasText: kind })
  await expect(row).toContainText(/35\sс/)
  await expect(row).toContainText(oil)
})

test('скидка: без права цену можно только поднять', async ({ page, browser }) => {
  await login(page)
  const code = `30${RUN}303`
  const product = `Фильтр без скидки ${RUN}`
  await goods(page.request, 'filter', product, code, 60_000)
  // У владельца карандаш есть.
  await page.goto('/')
  await scan(page, code)
  const ownerLine = page.getByRole('listitem').filter({ hasText: product })
  await expect(ownerLine.getByRole('button', { name: 'Изменить цену' })).toBeVisible()
  await ownerLine.getByRole('button', { name: 'Удалить строку' }).click()

  const admin = `kassa${RUN}`
  const password = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  await apiPost(page.request, '/users', { login: admin, password, full_name: `Касса ${RUN}`, role: 'admin' })
  const ctx = await browser.newContext()
  const adminPage = await ctx.newPage()
  await login(adminPage, admin, password)
  await adminPage.goto('/')
  await scan(adminPage, code)
  const line = adminPage.getByRole('listitem').filter({ hasText: product })
  await expect(line).toBeVisible()
  // Поле цены правится: ниже прайса — подсказка и «Провести» не даст, выше — можно (ADR-097).
  const price = line.getByLabel('Цена', { exact: true })
  await expect(price).toBeEditable()
  await price.fill('500')
  await expect(line.getByTestId('price-below-list')).toBeVisible()
  await expect(adminPage.getByRole('status').filter({ hasText: 'цена ниже каталожной' })).toBeVisible()
  await price.fill('700')
  await expect(line.getByTestId('price-below-list')).toHaveCount(0)
  await expect(adminPage.getByRole('status').filter({ hasText: 'цена ниже каталожной' })).toHaveCount(0)
  await ctx.close()
})
