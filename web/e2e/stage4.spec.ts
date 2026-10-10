// Расчёт сотрудника за день и месяц, ревизия склада сканером (SPEC-07, SPEC-15).
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2e4' }
const RUN = Date.now().toString().slice(-8)

interface ShiftRow {
  id: string
  counted_tyiyn: number | null
}

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

/** Открытая смена: смена одна в день, при повторном прогоне владелец переоткрывает сегодняшнюю. */
async function ensureOpenShift(request: APIRequestContext, cashierId: string): Promise<void> {
  if (await apiGet<ShiftRow | null>(request, '/shifts/current')) return
  const opened = await request.post('/api/v1/shifts/open', {
    data: { op_id: crypto.randomUUID(), cashier_employee_id: cashierId },
    headers: DEVICE,
  })
  if (opened.ok()) return
  const today = (await apiGet<ShiftRow[]>(request, '/shifts')).find((s) => s.counted_tyiyn !== null)
  await apiPost(request, `/shifts/${today?.id}/reopen`, { op_id: crypto.randomUUID(), reason: 'сквозной тест' })
}

async function product(request: APIRequestContext, name: string, code: string, price: number, cost: number, qty: number) {
  const cat = await apiPost<{ id: string }>(request, '/categories', { name: `Кат ${name}`, kind: 'other' })
  const p = await apiPost<{ id: string }>(request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name,
    barcodes: [code],
    sale_price_tyiyn: price,
  })
  await apiPost(request, '/receipts', { op_id: crypto.randomUUID(), lines: [{ product_id: p.id, qty, cost_tyiyn: cost * qty }] })
  return { ...p, category: `Кат ${name}` }
}

test('расчёт кассира: 2 % за день, выплата, оклад за месяц', async ({ page }) => {
  await login(page)
  const cashier = `Кассир расчёта ${RUN}`
  const emp = await apiPost<{ id: string }>(page.request, '/employees', { full_name: cashier, is_cashier: true })
  await ensureOpenShift(page.request, emp.id)
  const p = await product(page.request, `Щётка расчёт ${RUN}`, `25${RUN}301`, 100_000, 50_000, 5)
  // Чек на 1000 с при закупке 500 с: валовая 500 с, кассиру 2 % = 10 с.
  await apiPost(page.request, '/sales', {
    op_id: crypto.randomUUID(),
    sale_type: 'takeaway',
    cashier_id: emp.id,
    lines: [{ kind: 'piece', gift: false, product_id: p.id, qty: 1, unit_price_tyiyn: 100_000 }],
    payments: [{ method: 'cash', amount_tyiyn: 100_000 }],
  })

  await page.goto('/payroll')
  const row = page.getByRole('row', { name: new RegExp(cashier) })
  await expect(row).toContainText(/10,00/)
  await row.getByRole('button', { name: 'Выплатить' }).click()
  await page.getByRole('button', { name: 'Выплатить', exact: true }).last().click()
  await expect(page.getByText('Выплачено из кассы')).toBeVisible()

  await page.getByRole('button', { name: 'За месяц' }).click()
  const month = page.getByRole('row', { name: new RegExp(cashier) })
  await expect(month.getByText('не начислен')).toBeVisible()
  await month.getByRole('button', { name: 'Оклад' }).click()
  await expect(page.getByRole('textbox').last()).toHaveValue(/30\s?000/)
  await page.getByRole('button', { name: 'Начислить' }).click()
  await expect(page.getByText('Оклад начислен')).toBeVisible()
  await expect(month).toContainText(/30\s000,00/)
  // На конец месяца: оклад 30 000 + 10 − выплачено 10.
  await expect(month).toContainText(/30\s000,00/)
})

test('ревизия: сканер считает, владелец проводит, остаток выровнен', async ({ page }) => {
  await login(page)
  const code = `25${RUN}302`
  const p = await product(page.request, `Лампа ревизия ${RUN}`, code, 50_000, 20_000, 5)

  await page.goto('/revision')
  await page.getByLabel('Что пересчитываем').selectOption({ label: p.category })
  await page.getByRole('button', { name: 'Начать ревизию' }).click()
  await expect(page.getByRole('heading', { name: /Ревизия № \d+/ })).toBeVisible()
  // Нашли только 3 из 5: три скана.
  const scanner = page.getByPlaceholder('Сканируйте штрихкод')
  for (let i = 0; i < 3; i++) {
    await scanner.fill(code)
    await scanner.press('Enter')
    await expect(page.getByRole('row', { name: new RegExp(p.category === '' ? '' : `Лампа ревизия ${RUN}`) })).toContainText(`−${5 - (i + 1)} шт`)
  }
  await page.getByRole('button', { name: 'Провести' }).click()
  await page.getByLabel(/Кто пересчитывал/).fill('Айбек, плановая')
  await page.getByRole('button', { name: 'Провести' }).last().click()
  await expect(page.getByText('Ревизия проведена')).toBeVisible()
  await expect(page.getByText(/Итог ревизии в деньгах: −400,00/)).toBeVisible()
  const stock = await apiGet<{ id: string; stock_qty: number }[]>(page.request, `/stock?q=${encodeURIComponent(`Лампа ревизия ${RUN}`)}`)
  expect(stock[0].stock_qty).toBe(3)
})

test('кабинет юрлица: вход по ИНН, смена пароля, долг и покупки', async ({ page, browser }) => {
  await login(page)
  const inn = `9${RUN}12345`
  const name = `ОсОО Кабинет ${RUN}`
  const party = await apiPost<{ id: string }>(page.request, '/parties', { name, kind: 'company', inn })
  await apiPost(page.request, '/debts/adjust', { op_id: crypto.randomUUID(), party_id: party.id, amount_tyiyn: 250_000, comment: 'из тетради' })

  // Клиент заходит из своего браузера: входа точки у него нет.
  const ctx = await browser.newContext()
  const client = await ctx.newPage()
  await client.goto('/cabinet')
  await client.getByLabel('ИНН организации').fill(inn)
  await client.getByLabel(/^Пароль/).fill('avtodom2026')
  await client.getByRole('button', { name: 'Войти' }).click()
  await expect(client.getByText('Это первый вход')).toBeVisible()
  const pass = client.locator('input[type=password]')
  await pass.nth(0).fill('avtodom2026')
  await pass.nth(1).fill('taxi-secret-9')
  await pass.nth(2).fill('taxi-secret-9')
  await client.getByRole('button', { name: 'Сменить пароль' }).click()
  await expect(client.getByText(name)).toBeVisible()
  await expect(client.getByText('Ваш долг')).toBeVisible()
  await expect(client.getByText(/2\s500,00/).first()).toBeVisible()
  await client.getByRole('button', { name: 'Акт сверки' }).click()
  await expect(client.getByText('Корректировка долга')).toBeVisible()
  await ctx.close()

  // Владелец видит, что клиент сменил пароль.
  await page.goto(`/clients/${party.id}`)
  await expect(page.getByText(/клиент сменил пароль/)).toBeVisible()
})

test('аккумуляторы на вес: приём из кассы и продажа не дешевле средней', async ({ page }) => {
  await login(page)
  const cashier = `Кассир АКБ ${RUN}`
  const emp = await apiPost<{ id: string }>(page.request, '/employees', { full_name: cashier, is_cashier: true })
  await ensureOpenShift(page.request, emp.id)
  await apiPost(page.request, '/cash/movements', { op_id: crypto.randomUUID(), kind: 'cash_in', amount_tyiyn: 500_000, comment: 'размен' })

  await page.goto('/')
  await page.getByRole('button', { name: 'Услуги' }).click()
  await page.getByRole('button', { name: /Приём аккумуляторов/ }).click()
  await page.getByLabel('Вес, кг').fill('10')
  await page.getByLabel('Цена за кг, с').fill('100')
  await expect(page.getByText(/выдать 1\s000,00/)).toBeVisible()
  await page.getByRole('button', { name: 'Принять и выдать деньги' }).click()
  await expect(page.getByText(/выдано из кассы 1\s000,00/)).toBeVisible()

  await page.getByRole('button', { name: 'Услуги' }).click()
  await page.getByRole('button', { name: /Аккумуляторы на вес/ }).click()
  await expect(page.getByText(/дешевле не продать/)).toBeVisible()
  await page.locator('#cashier-select').selectOption({ label: cashier })
  await page.getByLabel('Вес, кг', { exact: true }).last().fill('4')
  await page.getByLabel(/^Цена за кг/).last().fill('150')
  await page.getByRole('button', { name: 'Провести чек' }).click()
  await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()
})

test('масляная книжка: замена с пробегом, следующая по интервалу', async ({ page }) => {
  await login(page)
  const cashier = `Кассир книжки ${RUN}`
  const master = `Мастер книжки ${RUN}`
  const emp = await apiPost<{ id: string }>(page.request, '/employees', { full_name: cashier, is_cashier: true })
  const mst = await apiPost<{ id: string }>(page.request, '/employees', { full_name: master, is_master: true })
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Масла книжки ${RUN}`, kind: 'oil' })
  const oil = await apiPost<{ id: string }>(page.request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Totachi книжка ${RUN}`,
    container_ml: 4000,
    sale_price_tyiyn: 700_000,
  })
  await apiPost(page.request, '/receipts', { op_id: crypto.randomUUID(), lines: [{ product_id: oil.id, qty: 8000, cost_tyiyn: 1_040_000 }] })
  const party = await apiPost<{ id: string }>(page.request, '/parties', { name: `Книжкин ${RUN}`, kind: 'person' })
  const plate = `01KG${RUN.slice(-3)}AAA`
  const vehicle = await apiPost<{ id: string }>(page.request, `/parties/${party.id}/vehicles`, { plate })
  await apiPost(page.request, '/sales', {
    op_id: crypto.randomUUID(),
    sale_type: 'service',
    cashier_id: emp.id,
    master_id: mst.id,
    party_id: party.id,
    vehicle_id: vehicle.id,
    mileage_km: 85_000,
    lines: [{ kind: 'container', gift: false, product_id: oil.id, qty: 1, unit_price_tyiyn: 700_000 }],
    payments: [{ method: 'cash', amount_tyiyn: 700_000 }],
  })

  await page.goto(`/clients/${party.id}`)
  await expect(page.getByRole('heading', { name: 'Масляная книжка' })).toBeVisible()
  await expect(page.getByText(plate).first()).toBeVisible()
  await expect(page.getByText(/на 93\s000 км/)).toBeVisible()
  await expect(page.getByText(`Totachi книжка ${RUN} · 1 кан. × 4 л`)).toBeVisible()
  // Интервал машины 10 000 км — следующая замена пересчитывается.
  await page.getByRole('button', { name: 'Интервал' }).click()
  await page.getByLabel('Каждые, км').fill('10000')
  await page.getByRole('button', { name: 'Сохранить' }).click()
  await expect(page.getByText(/на 95\s000 км/)).toBeVisible()
})

test('аналог фильтра вместо отсутствующего и доставка в чеке', async ({ page }) => {
  await login(page)
  const cashier = `Кассир аналогов ${RUN}`
  await apiPost(page.request, '/employees', { full_name: cashier, is_cashier: true })
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Фильтры аналогов ${RUN}`, kind: 'filter' })
  const code = `24${RUN}401`
  // Оригинала нет на складе, аналог Mann есть.
  const original = await apiPost<{ id: string }>(page.request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Toyota ориг ${RUN}`,
    article: `TY-${RUN}`,
    barcodes: [code],
    sale_price_tyiyn: 70_000,
  })
  const mann = await apiPost<{ id: string }>(page.request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Mann аналог ${RUN}`,
    sale_price_tyiyn: 60_000,
  })
  await apiPost(page.request, '/receipts', { op_id: crypto.randomUUID(), lines: [{ product_id: mann.id, qty: 5, cost_tyiyn: 175_000 }] })
  const res = await page.request.put(`/api/v1/products/${mann.id}/cross`, { data: { codes: [`ty ${RUN}`] }, headers: DEVICE })
  expect(res.ok()).toBeTruthy()
  expect(original.id).toBeTruthy()

  await page.goto('/')
  const picker = page.getByPlaceholder(/Сканируйте штрихкод/)
  await picker.fill(code)
  await picker.press('Enter')
  await expect(page.getByText('нет на складе — есть аналог:')).toBeVisible()
  await page.getByRole('button', { name: new RegExp(`Mann аналог ${RUN}`) }).click()
  await expect(page.locator('li').filter({ hasText: `Mann аналог ${RUN}` }).first()).toBeVisible()
  // Связь в обе стороны: теперь оригинал — аналог для Mann.
  await expect(page.getByRole('button', { name: new RegExp(`Toyota ориг ${RUN} · 0 шт`) })).toBeVisible()

  await page.locator('#cashier-select').selectOption({ label: cashier })
  await page.getByLabel('Доставка (бесплатно)').check()
  await page.locator('#delivery-address').fill('г. Бишкек, ул. Ахунбаева 98')
  await page.getByRole('button', { name: 'Провести чек' }).click()
  await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()
  await page.getByRole('link', { name: 'Открыть', exact: true }).click()
  await expect(page.getByText('Доставка: г. Бишкек, ул. Ахунбаева 98')).toBeVisible()
})

test('новый клиент в кассе: физлицо или юрлицо выбирают явно', async ({ page }) => {
  await login(page)
  await page.goto('/')
  const pin = `2${RUN}12345`.slice(0, 14).padEnd(14, '0')
  // 14 цифр — это и ПИН физлица, и ИНН фирмы: касса не угадывает, а спрашивает.
  await page.locator('[data-client-input]').fill(pin)
  await page.getByRole('button', { name: `+ Новый клиент «${pin}»` }).click()
  await expect(page.getByLabel('ПИН (14 цифр с паспорта)')).toHaveValue(pin)
  const create = page.getByRole('button', { name: 'Создать' })
  await expect(page.getByRole('status').filter({ hasText: 'физлицо или юрлицо' })).toBeVisible()
  await expect(create).toBeDisabled()
  await page.getByRole('button', { name: /^Физлицо/ }).click()
  await page.locator('#client-name').fill(`Физлицов ${RUN}`)
  await create.click()
  await expect(page.getByText(`Физлицов ${RUN}`)).toBeVisible()
  await expect(page.getByText(`ПИН ${pin}`)).toBeVisible()
  // Ошиблись — тип меняется прямо в чеке.
  await page.getByRole('button', { name: 'это юрлицо' }).click()
  await expect(page.getByRole('button', { name: 'это физлицо' })).toBeVisible()
  await expect(page.getByText(`ИНН ${pin}`)).toBeVisible()
})
