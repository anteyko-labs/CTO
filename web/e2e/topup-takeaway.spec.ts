// Доливка на вынос (SPEC-36 §4–5, ADR-118): услуга-доливка идёт в чек «На вынос» без клиента и
// машины, мастер по желанию; в «Виде замены» доливок нет, замены — первыми; 1 л моторного масла на
// вынос — без вопроса «Это замена масла?»; флажок — на экране «Услуги».
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e361' }
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
}

interface SaleOut {
  id: string
  number: number
  sale_type: string
  master_fee_tyiyn: number
  party_id?: string | null
}

test('доливка на вынос: без клиента, мастер по желанию, без вопроса о замене', async ({ page }) => {
  await login(page)
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Моторные масла доливки ${RUN}`, kind: 'oil' })
  const code = `47${RUN}361`
  const oil = await apiPost<{ id: string }>(page.request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Масло доливки 5W-30 ${RUN}`,
    container_ml: 4000,
    barcodes: [code],
    sale_price_tyiyn: 200_000,
  })
  const accounts = await apiGet<Account[]>(page.request, '/cash/accounts')
  const safe = accounts.find((a) => a.kind === 'safe' && a.name === 'Сейф владельца') ?? accounts.find((a) => a.kind === 'safe')
  await apiPost(page.request, '/receipts', { op_id: crypto.randomUUID(), paid_from: safe!.id, lines: [{ product_id: oil.id, qty: 8000, cost_tyiyn: 300_000 }] })
  const cashier = await apiPost<{ id: string }>(page.request, '/employees', { full_name: `Кассир доливки ${RUN}`, is_cashier: true, pin: '1234' })
  const master = `Мастер доливки ${RUN}`
  await apiPost(page.request, '/employees', { full_name: master, is_master: true })
  const topupName = `Доливка в мотор ${RUN}`
  const topup = await apiPost<{ id: string; takeaway: boolean }>(page.request, '/services', { name: topupName, price_tyiyn: 10_000, master_fee_tyiyn: 3_000 })
  expect(topup.takeaway).toBe(true)
  const changeName = `Замена масла мотор ${RUN}`
  await apiPost(page.request, '/services', { name: changeName, price_tyiyn: 50_000, master_fee_tyiyn: 3_000 })
  const cur = await apiGet<unknown>(page.request, '/shifts/current')
  if (!cur) await apiPost(page.request, '/shifts/open', { op_id: crypto.randomUUID(), cashier_employee_id: cashier.id, cashier_pin: '1234' })

  // Флажок на экране «Услуги»: в списке — «на вынос», в карточке — отмечен.
  await page.goto('/services')
  await page.getByPlaceholder('Поиск').fill(RUN)
  const row = page.getByRole('row', { name: new RegExp(topupName) })
  await expect(row.getByText('на вынос')).toBeVisible()
  await row.getByText(topupName).click()
  await expect(page.getByTestId('service-takeaway').getByRole('checkbox')).toBeChecked()
  await page.keyboard.press('Escape')

  const sell = async (withMaster: boolean): Promise<SaleOut> => {
    await page.goto('/')
    const picker = page.getByPlaceholder(PICKER)
    await picker.fill(code)
    await picker.press('Enter')
    await expect(page.getByText(`Масло доливки 5W-30 ${RUN}`)).toBeVisible()
    // 1 л розливом — доливка.
    await page.getByRole('button', { name: 'Розлив, л' }).click()
    await page.getByRole('button', { name: 'Услуги', exact: true }).click()
    const dialog = page.getByRole('dialog', { name: 'Услуги' })
    await dialog.getByPlaceholder('Найти услугу').fill(topupName)
    await dialog.getByRole('button', { name: new RegExp(topupName) }).click()
    // Чек остаётся «На вынос»: клиента и вида замены не спрашивают, мастер — по желанию.
    await expect(page.getByRole('button', { name: 'На вынос' })).toHaveClass(/bg-sky-600/)
    await expect(page.getByTestId('service-kinds')).toHaveCount(0)
    const masterSelect = page.locator('#master-select')
    await expect(masterSelect).toBeVisible()
    await expect(masterSelect).toHaveValue('')
    if (withMaster) await masterSelect.selectOption({ label: master })
    const posted = page.waitForResponse((r) => r.url().endsWith('/api/v1/sales') && r.request().method() === 'POST')
    await page.getByRole('button', { name: 'Провести чек' }).first().click()
    const res = await posted
    expect(res.ok(), await res.text()).toBeTruthy()
    // Вопроса «Это замена масла?» нет: масла меньше канистры.
    await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()
    await expect(page.getByTestId('oil-question')).toHaveCount(0)
    return (await res.json()) as SaleOut
  }

  const plain = await sell(false)
  expect(plain.sale_type).toBe('takeaway')
  expect(plain.party_id ?? null).toBeNull()
  expect(plain.master_fee_tyiyn).toBe(0)
  const withMaster = await sell(true)
  expect(withMaster.sale_type).toBe('takeaway')
  expect(withMaster.master_fee_tyiyn).toBe(3_000)

  // «В сервис» — окно «Вид замены» без доливки; доливка в чеке остаётся после «Готово».
  await page.goto('/')
  await page.getByRole('button', { name: 'Услуги', exact: true }).click()
  const services = page.getByRole('dialog', { name: 'Услуги' })
  await services.getByPlaceholder('Найти услугу').fill(topupName)
  await services.getByRole('button', { name: new RegExp(topupName) }).click()
  await page.getByRole('button', { name: 'В сервис' }).click()
  const kinds = page.getByRole('dialog', { name: 'Вид замены' })
  await expect(kinds).toBeVisible()
  await kinds.getByPlaceholder('Найти услугу').fill(RUN)
  await expect(kinds.getByRole('button', { name: new RegExp(changeName) })).toBeVisible()
  await expect(kinds.getByRole('button', { name: new RegExp(topupName) })).toHaveCount(0)
  await kinds.getByRole('button', { name: new RegExp(changeName) }).click()
  await kinds.getByRole('button', { name: 'Готово' }).click()
  await expect(page.getByTestId('service-kinds').getByText(changeName)).toBeVisible()
  await expect(page.getByTestId('cart').getByText(topupName)).toBeVisible()
})

test('режим кассы 1366×768: доливка на вынос с мастером помещается без прокрутки', async ({ page }) => {
  await page.setViewportSize({ width: 1366, height: 768 })
  await login(page)
  const name = `Доливка антифриза ${RUN}`
  await apiPost(page.request, '/services', { name, price_tyiyn: 5_000, master_fee_tyiyn: 3_000 })
  await apiPost(page.request, '/employees', { full_name: `Мастер кассы доливки ${RUN}`, is_master: true })
  await page.goto('/')
  await page.evaluate(() => localStorage.setItem('avtodom.cashier_mode', '1'))
  await page.reload()
  await expect(page.getByTestId('cashier-mode-bar')).toBeVisible()
  await page.getByRole('button', { name: 'Услуги', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Услуги' })
  await dialog.getByPlaceholder('Найти услугу').fill(name)
  await dialog.getByRole('button', { name: new RegExp(name) }).click()
  const masterSelect = page.locator('#master-select')
  await expect(masterSelect).toBeVisible()
  expect((await masterSelect.boundingBox())!.height).toBeGreaterThanOrEqual(56)
  const fits = await page.evaluate(() => {
    const el = document.scrollingElement ?? document.documentElement
    return el.scrollHeight <= window.innerHeight + 1 && el.scrollWidth <= window.innerWidth + 1
  })
  expect(fits).toBe(true)
  const submit = page.getByRole('button', { name: 'Провести чек' }).first()
  const box = (await submit.boundingBox())!
  expect(box.y + box.height).toBeLessThanOrEqual(769)
  await page.evaluate(() => localStorage.removeItem('avtodom.cashier_mode'))
})
