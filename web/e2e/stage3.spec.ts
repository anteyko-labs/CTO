// Этап 3 в браузере: услуги и мастер, продажа в долг с распиской, оплата долга, подарки,
// перелив масла, акт сверки и запрет продажи ниже закупки (SPEC-10, SPEC-11, SPEC-14, ADR-043).
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2e3' }
const PICKER = /Сканируйте штрихкод/

/** Уникальная метка прогона, чтобы тесты не мешали данным в базе. */
const RUN = Date.now().toString().slice(-8)
/** Сумма как на экране: пробелы в разрядах и перед «с» неразрывные. */
const money = (text: string) => new RegExp(text.replace(/ /g, '\\s'))

interface Party {
  id: string
  name: string
  inn: string
  balance_tyiyn: number
}

interface ShiftRow {
  id: string
  counted_tyiyn: number | null
}

interface Sale {
  id: string
  number: number
  total_tyiyn: number
  master_fee_tyiyn: number
}

/**
 * Печать документов идёт через скрытый iframe и его window.print(). Подменяем print у окна iframe
 * в момент обращения к contentWindow: вызов записывает HTML документа в window.__prints.
 */
const PRINT_STUB = () => {
  const w = window as unknown as { __prints: string[] }
  w.__prints = []
  window.print = () => {
    w.__prints.push(document.documentElement.outerHTML)
  }
  const desc = Object.getOwnPropertyDescriptor(HTMLIFrameElement.prototype, 'contentWindow')
  const getter = desc?.get
  if (!getter) return
  Object.defineProperty(HTMLIFrameElement.prototype, 'contentWindow', {
    configurable: true,
    get(this: HTMLIFrameElement) {
      const win = getter.call(this) as Window | null
      if (win) {
        try {
          win.print = () => {
            w.__prints.push(win.document.documentElement.outerHTML)
          }
        } catch {
          // Окно чужого источника — не наше.
        }
      }
      return win
    },
  })
}

const prints = async (page: Page) =>
  (await page.evaluate(() => (window as unknown as { __prints?: string[] }).__prints ?? [])).map((h) => h.replace(/&nbsp;/g, ' '))

async function login(page: Page) {
  await page.addInitScript(PRINT_STUB)
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

async function apiSend<T>(request: APIRequestContext, method: 'POST' | 'PUT' | 'PATCH', path: string, data: unknown): Promise<T> {
  const res = await request.fetch(`/api/v1${path}`, { method, data, headers: DEVICE })
  expect(res.ok(), `${method} ${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

const apiPost = <T>(request: APIRequestContext, path: string, data: unknown) => apiSend<T>(request, 'POST', path, data)

/** Открытая смена: смена одна в день, при повторном прогоне владелец переоткрывает сегодняшнюю. */
async function ensureOpenShift(request: APIRequestContext, cashierId: string): Promise<void> {
  const current = await apiGet<ShiftRow | null>(request, '/shifts/current')
  if (current) return
  const opened = await request.post('/api/v1/shifts/open', {
    data: { op_id: crypto.randomUUID(), cashier_employee_id: cashierId },
    headers: DEVICE,
  })
  if (opened.ok()) return
  expect(opened.status()).toBe(409)
  const list = await apiGet<ShiftRow[]>(request, '/shifts')
  const today = list.find((s) => s.counted_tyiyn !== null)
  expect(today, 'сегодняшняя закрытая смена').toBeTruthy()
  await apiPost(request, `/shifts/${today?.id}/reopen`, { op_id: crypto.randomUUID(), reason: 'сквозной тест' })
}

async function employee(request: APIRequestContext, name: string, roles: { is_cashier?: boolean; is_master?: boolean }) {
  return apiPost<{ id: string }>(request, '/employees', { full_name: name, ...roles })
}

/** Штучный товар с приходом: цена продажи и закупка за штуку в тыйынах. */
async function pieceProduct(request: APIRequestContext, name: string, code: string, price: number, cost: number, qty = 10) {
  const cat = await apiPost<{ id: string }>(request, '/categories', { name: `Кат ${name}`, kind: 'other' })
  const p = await apiPost<{ id: string }>(request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name,
    barcodes: [code],
    sale_price_tyiyn: price,
  })
  await apiPost(request, '/receipts', { op_id: crypto.randomUUID(), lines: [{ product_id: p.id, qty, cost_tyiyn: cost * qty }] })
  return p
}

/** Масло: канистра container_ml, приход ml по cost_per_l тыйын за литр. */
async function oilProduct(
  request: APIRequestContext,
  name: string,
  opts: { code?: string; container_ml: number; price: number; pour?: number; ml: number; cost_per_l: number },
) {
  const cat = await apiPost<{ id: string }>(request, '/categories', { name: `Масла ${name}`, kind: 'oil' })
  const p = await apiPost<{ id: string }>(request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name,
    container_ml: opts.container_ml,
    barcodes: opts.code ? [opts.code] : [],
    sale_price_tyiyn: opts.price,
    pour_price_per_l_tyiyn: opts.pour ?? null,
  })
  await apiPost(request, '/receipts', {
    op_id: crypto.randomUUID(),
    lines: [{ product_id: p.id, qty: opts.ml, cost_tyiyn: (opts.cost_per_l * opts.ml) / 1000 }],
  })
  return p
}

/** Продажа в долг через API: клиенту нужен долг для оплаты и акта сверки. */
async function debtSale(request: APIRequestContext, cashierId: string, partyId: string, productId: string, price: number) {
  return apiPost<Sale>(request, '/sales', {
    op_id: crypto.randomUUID(),
    client_time: new Date().toISOString(),
    sale_type: 'takeaway',
    cashier_id: cashierId,
    master_id: null,
    party_id: partyId,
    contact_id: null,
    vehicle_id: null,
    comment: '',
    lines: [{ kind: 'piece', gift: false, product_id: productId, service_id: null, qty: 1, unit_price_tyiyn: price }],
    payments: [{ method: 'debt', amount_tyiyn: price }],
  })
}

async function scan(page: Page, code: string) {
  const picker = page.getByPlaceholder(PICKER)
  await picker.fill(code)
  await picker.press('Enter')
}

const payButton = (page: Page, label: string) => page.locator('[data-pay]').getByRole('button', { name: label, exact: true })

test('услуга «масло клиента»: ставка мастера из услуги, без ставки за замену', async ({ page }) => {
  await login(page)
  const cashier = `Кассир услуг ${RUN}`
  const master = `Мастер услуг ${RUN}`
  await employee(page.request, cashier, { is_cashier: true })
  const m = await employee(page.request, master, { is_master: true })
  const service = `Замена масла (масло клиента) ${RUN}`

  await page.goto('/')
  await page.getByRole('button', { name: 'Услуги' }).click()
  await expect(page.getByRole('heading', { name: 'Услуги' })).toBeVisible()
  await page.getByRole('button', { name: '+ Новая услуга' }).click()
  await page.locator('#service-name').fill(service)
  await page.locator('#service-price').fill('400')
  await page.locator('#service-fee').fill('200')
  await expect(page.getByText(money('В кассу останется 200 с'))).toBeVisible()
  await page.getByRole('button', { name: 'Создать и добавить в чек' }).click()

  // Услуга в чеке, чек сам стал «в сервис».
  await expect(page.getByRole('heading', { name: 'Услуги' })).toBeHidden()
  await expect(page.getByRole('listitem').filter({ hasText: service })).toBeVisible()
  await expect(page.getByRole('button', { name: 'В сервис' })).toHaveClass(/bg-sky-600/)
  await page.locator('#cashier-select').selectOption({ label: cashier })
  await page.locator('#master-select').selectOption({ label: master })
  await page.getByRole('button', { name: 'Без сдачи' }).click()
  await page.getByRole('button', { name: 'Провести чек' }).click()
  await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()

  const href = await page.getByRole('link', { name: 'Открыть', exact: true }).getAttribute('href')
  const sale = await apiGet<Sale>(page.request, href ?? '')
  expect(sale.total_tyiyn).toBe(40000)
  // 200 с из услуги; ставка за замену (30 с) не добавляется — товара в чеке нет.
  expect(sale.master_fee_tyiyn).toBe(20000)
  const day = await apiGet<{ employee_id: string; service_fee_tyiyn: number }[]>(page.request, '/payroll/day')
  expect(day.find((r) => r.employee_id === m.id)?.service_fee_tyiyn).toBe(20000)
})

test('продажа в долг: ПИН из кассы и печать расписки', async ({ page }) => {
  await login(page)
  const cashier = `Кассир долга ${RUN}`
  await employee(page.request, cashier, { is_cashier: true })
  const code = `27${RUN}101`
  const product = `Фильтр в долг ${RUN}`
  await pieceProduct(page.request, product, code, 125_000, 60_000)
  const client = `Долгов Тест ${RUN}`
  const pin = `2${RUN}12345`
  await apiPost<Party>(page.request, '/parties', { role: 'customer', kind: 'person', name: client, phone: '+996555000111' })
  const seller = `ИП Асанов ${RUN}`
  await apiSend(page.request, 'PUT', '/settings/debt-docs', {
    seller: { name: seller, inn: '01234567890123', address: 'г. Бишкек, ул. Тестовая 1', phone: '+996 312 000000', director: 'Асанов А. А.', bank: '', city: 'Бишкек' },
    person: null,
    company: null,
  })

  await page.goto('/')
  await scan(page, code)
  await expect(page.getByText(product)).toBeVisible()
  await page.locator('#cashier-select').selectOption({ label: cashier })
  await page.locator('[data-client-input]').fill(client)
  await page.getByRole('button', { name: new RegExp(`^${client}`) }).click()
  await payButton(page, 'В долг').click()

  // Без ПИН документ о долге не составить: касса просит его и не проводит чек.
  const submit = page.getByRole('button', { name: 'Провести чек' })
  await expect(page.getByRole('status').filter({ hasText: 'ПИН / ИНН клиента для документа о долге' })).toBeVisible()
  await expect(submit).toBeDisabled()
  await page.locator('#debt-inn').fill(pin)
  await page.getByRole('button', { name: 'Записать' }).click()
  await expect(page.getByText(`ПИН ${pin}`)).toBeVisible()
  await expect(page.getByText('ПИН / ИНН клиента для документа о долге')).toHaveCount(0)
  await submit.click()
  await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()

  await expect.poll(async () => (await prints(page)).length, { message: 'расписка ушла на печать' }).toBeGreaterThan(0)
  const html = (await prints(page)).join('\n')
  expect(html).toContain('РАСПИСКА')
  expect(html).toContain(client)
  expect(html).toContain(pin)
  expect(html).toContain(seller)
  expect(html).toContain('Одна тысяча двести пятьдесят сом 00 тыйын')
  // Два экземпляра: покупателю и точке.
  expect(html.match(/РАСПИСКА/g)?.length).toBe(2)
  const saved = await apiGet<Party[]>(page.request, `/parties?role=customer&q=${encodeURIComponent(client)}`)
  expect(saved[0]?.inn).toBe(pin)
  expect(saved[0]?.balance_tyiyn).toBe(125_000)
})

test('повторный долг: в расписке прежний долг и общий итог', async ({ page }) => {
  // Взял в долг 400, пришёл снова и взял ещё 500: расписка показывает прежние 400 и итог 900.
  await login(page)
  const cashier = `Кассир повторного долга ${RUN}`
  const cashierId = (await employee(page.request, cashier, { is_cashier: true })).id
  await ensureOpenShift(page.request, cashierId)
  const code = `27${RUN}202`
  const product = `Свеча в долг ${RUN}`
  const productId = (await pieceProduct(page.request, product, code, 50_000, 20_000)).id
  const client = `Повторный Долг ${RUN}`
  const party = await apiPost<Party>(page.request, '/parties', {
    role: 'customer',
    kind: 'person',
    name: client,
    phone: '+996555000222',
    inn: `2${RUN}54321`,
  })
  // Первый долг — 400 с.
  await debtSale(page.request, cashierId, party.id, productId, 40_000)

  await page.goto('/')
  await scan(page, code)
  await expect(page.getByText(product)).toBeVisible()
  await page.locator('#cashier-select').selectOption({ label: cashier })
  await page.locator('[data-client-input]').fill(client)
  await page.getByRole('button', { name: new RegExp(`^${client}`) }).click()
  await payButton(page, 'В долг').click()
  await page.getByRole('button', { name: 'Провести чек' }).click()
  await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()

  await expect.poll(async () => (await prints(page)).length, { message: 'расписка ушла на печать' }).toBeGreaterThan(0)
  const html = (await prints(page)).join('\n')
  expect(html).toContain('Ранее полученное в долг')
  expect(html).toContain('Задолженность до этой покупки')
  expect(html).toMatch(/400,00/)
  expect(html).toMatch(/900,00/)
  expect(html).toContain('Девятьсот сом 00 тыйын')
})

test('оплата долга из кассы попадает в смену', async ({ page }) => {
  await login(page)
  const cashier = `Кассир оплаты ${RUN}`
  const emp = await employee(page.request, cashier, { is_cashier: true })
  await ensureOpenShift(page.request, emp.id)
  const prod = await pieceProduct(page.request, `Фильтр оплаты ${RUN}`, `25${RUN}202`, 100_000, 50_000)
  const client = `Должник Оплатов ${RUN}`
  const party = await apiPost<Party>(page.request, '/parties', { role: 'customer', kind: 'person', name: client, inn: `3${RUN}12345` })
  await debtSale(page.request, emp.id, party.id, prod.id, 100_000)

  await page.goto('/')
  await page.getByRole('button', { name: 'Принять оплату долга' }).click()
  await page.getByPlaceholder('Имя, телефон или ИНН должника').fill(client)
  await page.getByRole('button', { name: new RegExp(client) }).click()
  await expect(page.getByRole('heading', { name: `Оплата долга: ${client}` })).toBeVisible()
  await expect(page.locator('#repay-sum')).toHaveValue('1000')
  await page.locator('#repay-sum').fill('400')
  await page.getByRole('button', { name: 'Принять оплату', exact: true }).click()
  await expect(page.getByText(money('Оплата долга принята: 400 с'))).toBeVisible()

  await expect.poll(async () => (await apiGet<Party>(page.request, `/parties/${party.id}`)).balance_tyiyn).toBe(60_000)
  await page.goto('/shift')
  // Строка наличных смены: «Погашения долгов   + 400 с».
  await expect(page.locator('div').filter({ hasText: /^Погашения долгов\+\s[\d\s]+(,\d\d)?\sс$/ }).first()).toBeVisible()
})

test('подарок по порогу: 1 л розлива мало, канистра 4 л — спрашивает', async ({ page }) => {
  await login(page)
  const cashier = `Кассир подарков ${RUN}`
  await employee(page.request, cashier, { is_cashier: true })
  const oilCode = `24${RUN}303`
  const oil = await oilProduct(page.request, `Totachi 0W-20 ${RUN}`, {
    code: oilCode,
    container_ml: 4000,
    price: 300_000,
    pour: 80_000,
    ml: 40_000,
    cost_per_l: 50_000,
  })
  const gift = await pieceProduct(page.request, `Ароматизатор ${RUN}`, `23${RUN}404`, 15_000, 5_000)
  await apiPost(page.request, '/gift-rules', {
    trigger_product_id: oil.id,
    min_units: 3000,
    items: [{ gift_product_id: gift.id, gift_qty: 1 }],
  })

  await page.goto('/')
  // Правило подарка приходит после строки; держим ответ, пока кассир переключает строку на розлив 1 л.
  let release: () => void = () => undefined
  const held = new Promise<void>((r) => (release = r))
  await page.route('**/api/v1/gift-rules?*', async (route) => {
    await held
    await route.continue()
  })
  await scan(page, oilCode)
  const oilLine = page.getByRole('listitem').filter({ hasText: `Totachi 0W-20 ${RUN}` })
  await oilLine.getByRole('button', { name: 'Розлив, л' }).click()
  await expect(oilLine.getByRole('textbox').first()).toHaveValue('1')
  const rules = page.waitForResponse('**/api/v1/gift-rules?*')
  release()
  await rules
  await page.unroute('**/api/v1/gift-rules?*')
  await expect(page.getByRole('heading', { name: 'Выберите подарок' })).toHaveCount(0)

  // Канистра 4 л добавляется отдельной строкой: всего 5 л — порог 3 л набран.
  await scan(page, oilCode)
  await expect(page.getByRole('heading', { name: 'Выберите подарок' })).toBeVisible()
  await page.getByRole('button', { name: new RegExp(`Ароматизатор ${RUN}`) }).click()
  const giftLine = page.getByRole('listitem').filter({ hasText: `Ароматизатор ${RUN}` })
  await expect(giftLine).toContainText('подарок')
  // У подарка нет поля цены: «бесплатно», правится только количество.
  await expect(giftLine).toContainText('бесплатно')
  await expect(giftLine.getByRole('textbox')).toHaveCount(1)
  await expect(giftLine).toContainText(money('0 с'))

  await page.locator('#cashier-select').selectOption({ label: cashier })
  await page.getByRole('button', { name: 'Без сдачи' }).click()
  await page.getByRole('button', { name: 'Провести чек' }).click()
  await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()
  // 1 л по 800 + канистра 3000, подарок за ноль.
  await expect(page.getByText(money('Итог 3 800 с'))).toBeVisible()
})

test('подарок: обычный порядок кассира — скан, розлив 1 л, потом канистра', async ({ page }) => {
  await login(page)
  const oilCode = `22${RUN}505`
  const oil = await oilProduct(page.request, `Totachi 0W-20 Б ${RUN}`, {
    code: oilCode,
    container_ml: 4000,
    price: 300_000,
    pour: 80_000,
    ml: 40_000,
    cost_per_l: 50_000,
  })
  const gift = await pieceProduct(page.request, `Ароматизатор Б ${RUN}`, `21${RUN}606`, 15_000, 5_000)
  await apiPost(page.request, '/gift-rules', { trigger_product_id: oil.id, min_units: 3000, items: [{ gift_product_id: gift.id, gift_qty: 1 }] })

  await page.goto('/')
  await scan(page, oilCode)
  const oilLine = page.getByRole('listitem').filter({ hasText: `Totachi 0W-20 Б ${RUN}` })
  await expect(oilLine).toBeVisible()
  // Строка встаёт канистрой 4 л — порог набран сразу, окно появляется до того, как кассир выбрал розлив.
  // Клиенту нужен 1 л, кассир отвечает «Без подарка».
  const dialog = page.getByRole('heading', { name: 'Выберите подарок' })
  await expect(dialog).toBeVisible()
  await page.getByRole('button', { name: 'Без подарка' }).click()
  await oilLine.getByRole('button', { name: 'Розлив, л' }).click()
  await expect(oilLine.getByRole('textbox').first()).toHaveValue('1')
  await expect(dialog).toHaveCount(0)
  await scan(page, oilCode)
  await expect(page.getByRole('listitem').filter({ hasText: `Totachi 0W-20 Б ${RUN}` })).toHaveCount(2)
  await expect(dialog, 'после добавления канистры 4 л касса должна предложить подарок').toBeVisible()
})

test('перелив масла: 5 л по 100 в 50 л по 200 дают 190,91 за литр', async ({ page }) => {
  await login(page)
  const from = await oilProduct(page.request, `Остаток бочки ${RUN}`, { container_ml: 1000, price: 30_000, ml: 5_000, cost_per_l: 10_000 })
  const to = await oilProduct(page.request, `Основное масло ${RUN}`, { container_ml: 4000, price: 120_000, ml: 50_000, cost_per_l: 20_000 })

  await page.goto('/oil-transfer')
  await page.locator('#oil-from').selectOption(from.id)
  await page.locator('#oil-to').selectOption(to.id)
  await expect(page.locator('#oil-liters')).toHaveValue('5')
  await expect(page.getByText(money('55 л по 190,91 с за л'))).toBeVisible()
  await page.getByRole('button', { name: 'Перелить' }).click()
  await expect(page.getByText(money('себестоимость 190,91 с за литр'))).toBeVisible()
  const stock = await apiGet<{ id: string; stock_qty: number }>(page.request, `/products/${to.id}`)
  expect(stock.stock_qty).toBe(55_000)
})

test('акт сверки из карточки клиента', async ({ page }) => {
  await login(page)
  const emp = await employee(page.request, `Кассир акта ${RUN}`, { is_cashier: true })
  await ensureOpenShift(page.request, emp.id)
  const prod = await pieceProduct(page.request, `Фильтр акта ${RUN}`, `20${RUN}707`, 200_000, 100_000)
  const client = `Сверкин Акт ${RUN}`
  const party = await apiPost<Party>(page.request, '/parties', { role: 'customer', kind: 'person', name: client, inn: `4${RUN}12345` })
  const sale = await debtSale(page.request, emp.id, party.id, prod.id, 200_000)
  await apiPost(page.request, '/debts/repayments', { op_id: crypto.randomUUID(), party_id: party.id, amount_tyiyn: 50_000, method: 'cash', comment: '' })

  await page.goto(`/clients/${party.id}`)
  await page.getByRole('button', { name: 'Акт сверки' }).click()
  await page.getByRole('button', { name: 'Печать' }).click()
  await expect.poll(async () => (await prints(page)).length, { message: 'акт ушёл на печать' }).toBeGreaterThan(0)
  const html = (await prints(page)).join('\n')
  expect(html).toContain('АКТ СВЕРКИ')
  expect(html).toContain(client)
  expect(html).toMatch(/Сальдо на \d\d\.\d\d\.\d{4}/)
  expect(html).toContain(`Отгрузка товара, чек № ${sale.number}`)
  expect(html).toContain('Оплата наличными')
  // Обороты: дебет 2000, кредит 500; исходящее сальдо 1500 в пользу точки.
  expect(html).toMatch(money('2 000,00 с'))
  expect(html).toMatch(money('500,00 с'))
  expect(html).toMatch(money('1 500,00 с'))
  expect(html).toContain('Одна тысяча пятьсот сом 00 тыйын')
})

test('цена ниже закупочной: касса показывает отказ сервера, чек не проведён', async ({ page }) => {
  await login(page)
  const cashier = `Кассир цены ${RUN}`
  await employee(page.request, cashier, { is_cashier: true })
  const code = `19${RUN}808`
  const product = `Фильтр дешёвый ${RUN}`
  await pieceProduct(page.request, product, code, 50_000, 30_000)
  const before = (await apiGet<{ sales: Sale[] }>(page.request, '/sales')).sales.length

  await page.goto('/')
  await scan(page, code)
  const line = page.getByRole('listitem').filter({ hasText: product })
  await line.getByRole('textbox').nth(1).fill('100')
  await page.locator('#cashier-select').selectOption({ label: cashier })
  await page.getByRole('button', { name: 'Без сдачи' }).click()
  await page.getByRole('button', { name: 'Провести чек' }).click()
  await expect(page.getByText(/строка 1: цена ниже закупочной \(300\sс\), дешевле продать нельзя/)).toBeVisible()
  await expect(page.getByText(/Чек № .* проведён/)).toHaveCount(0)
  // Чек остался в кассе для исправления и не ушёл в очередь без сети.
  await expect(line).toBeVisible()
  expect((await apiGet<{ sales: Sale[] }>(page.request, '/sales')).sales.length).toBe(before)
})

test('замена в сервисе у физлица: машина, обязательный пробег, книжка с QR на печать', async ({ page }) => {
  await login(page)
  const cashier = `Кассир книжки ${RUN}`
  const master = `Мастер книжки ${RUN}`
  await employee(page.request, cashier, { is_cashier: true })
  await employee(page.request, master, { is_master: true })
  const client = `Частник ${RUN}`
  await apiSend(page.request, 'POST', '/parties', { name: client, kind: 'person', phone: `0555${RUN.slice(-6)}` })
  const plate = `01KG${RUN.slice(-3)}BBB`

  await page.goto('/')
  await page.getByRole('button', { name: 'Услуги' }).click()
  await page.getByRole('button', { name: '+ Новая услуга' }).click()
  await page.locator('#service-name').fill(`Замена масла (масло клиента) книжка ${RUN}`)
  await page.locator('#service-price').fill('400')
  await page.locator('#service-fee').fill('200')
  await page.getByRole('button', { name: 'Создать и добавить в чек' }).click()
  await page.locator('#cashier-select').selectOption({ label: cashier })
  await page.locator('#master-select').selectOption({ label: master })
  await expect(page.getByText(/Масляная книжка: выберите клиента и машину/)).toBeVisible()

  // У физлица тоже выбирается машина; новая заводится не выходя из чека.
  await page.locator('[data-client-input]').fill(client)
  await page.getByRole('button', { name: new RegExp(`^${client}`) }).click()
  await page.getByRole('button', { name: '+ новая машина' }).click()
  await page.getByLabel('Госномер').fill(plate)
  await page.getByRole('button', { name: 'Добавить' }).click()
  const submit = page.getByRole('button', { name: 'Провести чек' })
  await expect(page.getByRole('status').filter({ hasText: 'пробег машины — спросите у клиента' })).toBeVisible()
  await expect(submit).toBeDisabled()
  await page.locator('#mileage').fill('120500')
  await page.getByRole('button', { name: 'Без сдачи' }).click()
  await submit.click()
  await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()
  await expect(page.getByText(`Записано в масляную книжку · ${plate}`)).toBeVisible()
  await expect(page.getByText(/пробег 120\s500 км/)).toBeVisible()

  await expect.poll(async () => (await prints(page)).some((h) => h.includes('Масляная книжка')), { message: 'книжка ушла на печать' }).toBe(true)
  const html = (await prints(page)).find((h) => h.includes('Масляная книжка')) ?? ''
  expect(html).toContain(plate)
  expect(html).toContain('Замена масла (масло клиента) книжка')
  expect(html).toMatch(/120\s500 км/)
})
