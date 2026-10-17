// Доработки по проверке на точке в браузере (SPEC-27): аккумулятор в оплату чека и масляная
// книжка по каждому фильтру.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { pickCashier } from './cashier'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2e7' }
const RUN = Date.now().toString().slice(-8)
const money = (text: string) => new RegExp(text.replace(/ /g, '\\s'))

/** Печать идёт через скрытый iframe: подменяем print, HTML документа — в window.__prints. */
const PRINT_STUB = () => {
  const w = window as unknown as { __prints: string[] }
  w.__prints = []
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
  (await page.evaluate(() => (window as unknown as { __prints?: string[] }).__prints ?? [])).map((h) => h.replace(/&nbsp;| | /g, ' '))

async function login(page: Page) {
  await page.addInitScript(PRINT_STUB)
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'GET' | 'POST' | 'PUT' | 'PATCH', path: string, data?: unknown): Promise<T> {
  const res = await request.fetch(`/api/v1${path}`, { method, data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

/** Открытая смена: возврат наличными без неё не проводится (ADR-065). */
async function ensureOpenShift(request: APIRequestContext, cashierId: string): Promise<void> {
  if (await api(request, 'GET', '/shifts/current')) return
  await api(request, 'POST', '/shifts/open', { op_id: crypto.randomUUID(), cashier_employee_id: cashierId })
}

async function goods(request: APIRequestContext, categoryId: string, name: string, price: number, cost: number, extra: Record<string, unknown> = {}) {
  const p = await api<{ id: string }>(request, 'POST', '/products', {
    op_id: crypto.randomUUID(),
    category_id: categoryId,
    name,
    sale_price_tyiyn: price,
    ...extra,
  })
  const qty = extra.container_ml ? 8000 : 5
  await api(request, 'POST', '/receipts', { op_id: crypto.randomUUID(), paid_from: 'outside', lines: [{ product_id: p.id, qty, cost_tyiyn: cost * (extra.container_ml ? 2 : qty) }] })
  return p
}

test('аккумулятор в оплату: 10 000 с − 12 кг × 50 с, возврат аккумулятором', async ({ page }) => {
  await login(page)
  // Цена приёма — настройка филиала; после проверки возвращаем прежнюю.
  const before = await api<{ intake_per_kg_tyiyn: number }>(page.request, 'GET', '/batteries')
  await api(page.request, 'PUT', '/settings/batteries', { intake_per_kg_tyiyn: 5_000 })
  try {
    const cashier = `Кассир АКБ-оплата ${RUN}`
    const emp = await api<{ id: string }>(page.request, 'POST', '/employees', { full_name: cashier, is_cashier: true })
    const code = `27${RUN}777`
    const cat = await api<{ id: string }>(page.request, 'POST', '/categories', { name: `АКБ ${RUN}`, kind: 'battery' })
    const name = `Varta 60 ${RUN}`
    await goods(page.request, cat.id, name, 1_000_000, 600_000, { barcodes: [code], attrs: { polarity: 'обратная' } })

    await page.goto('/')
    await pickCashier(page, cashier)
    const picker = page.getByPlaceholder(/Сканируйте штрихкод/)
    await picker.fill(code)
    await picker.press('Enter')
    // Полярность в названии строки: «(П)» — обратная.
    await expect(page.getByText(`${name} (П)`)).toBeVisible()
    await page.getByRole('button', { name: 'Сдал аккумулятор' }).click()
    await page.locator('#battery-kg').fill('12')
    const box = page.getByTestId('battery-pay')
    await expect(box).toContainText(money('= 600 с'))
    // Цену за кг кассир меняет в чеке (ADR-078): 12 кг × 70 с = 840 с; вернём 50 с.
    await page.locator('#battery-per-kg').fill('70')
    await expect(box).toContainText(money('= 840 с'))
    await page.locator('#battery-per-kg').fill('50')
    await expect(box).toContainText(money('= 600 с'))
    await expect(box).toContainText(money('к оплате 9 400 с'))
    await page.getByRole('button', { name: 'Без сдачи' }).click()
    await expect(page.getByLabel('Получено, с')).toHaveValue('9400')
    await page.getByRole('button', { name: 'Провести чек' }).click()
    await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()
    await expect.poll(async () => (await prints(page)).length).toBe(0)
    await page.getByRole('button', { name: 'Печать чека' }).click()
    await expect.poll(async () => (await prints(page)).some((h) => /Аккумулятор 12 кг/.test(h))).toBe(true)

    // Чек: оплата аккумулятором с весом; возврат — 600 с аккумулятором, остальное деньгами.
    await page.getByRole('link', { name: 'Открыть', exact: true }).click()
    await expect(page.getByText(money('Аккумулятор 12 кг: 600 с'))).toBeVisible()
    await ensureOpenShift(page.request, emp.id)
    await page.getByRole('button', { name: 'Возврат' }).click()
    await page.getByRole('row', { name: new RegExp(name) }).getByRole('textbox').fill('1')
    await page.getByLabel('Причина').fill('не подошёл')
    await expect(page.getByText(money('600 с возвращается аккумулятором'))).toBeVisible()
    await page.getByRole('button', { name: 'Оформить возврат' }).click()
    await expect(page.getByRole('heading', { name: /^Возврат № \d+$/ })).toBeVisible()
    await expect(page.getByText(/Аккумулятор 12\sкг: −600\sс/)).toBeVisible()
  } finally {
    await api(page.request, 'PUT', '/settings/batteries', { intake_per_kg_tyiyn: before.intake_per_kg_tyiyn })
  }
})

test('масляная книжка по каждому фильтру: строка у категории, интервал у машины, печать', async ({ page }) => {
  await login(page)
  const emp = await api<{ id: string }>(page.request, 'POST', '/employees', { full_name: `Кассир фильтров ${RUN}`, is_cashier: true })
  const mst = await api<{ id: string }>(page.request, 'POST', '/employees', { full_name: `Мастер фильтров ${RUN}`, is_master: true })
  const oilCat = await api<{ id: string }>(page.request, 'POST', '/categories', { name: `Масла по строкам ${RUN}`, kind: 'oil' })
  const filterCat = await api<{ id: string }>(page.request, 'POST', '/categories', { name: `Фильтры масляные ${RUN}`, kind: 'filter' })
  const airName = `Воздушные ${RUN}`
  const airCat = await api<{ id: string }>(page.request, 'POST', '/categories', { name: airName, kind: 'filter' })

  // Владелец задаёт у категории строку книжки и интервал.
  await page.goto('/categories')
  await page.getByRole('row', { name: new RegExp(airName) }).click()
  await page.locator('#book-slot').selectOption({ label: 'Фильтр воздушный' })
  await page.locator('#book-km').fill('20000')
  await page.locator('#book-days').fill('60')
  await page.getByRole('button', { name: 'Сохранить' }).click()
  await expect(page.getByRole('row', { name: new RegExp(airName) })).toContainText('Фильтр воздушный')

  const oil = await goods(page.request, oilCat.id, `Масло строки ${RUN}`, 700_000, 520_000, { container_ml: 4000 })
  const filter = await goods(page.request, filterCat.id, `Фильтр W712 ${RUN}`, 50_000, 20_000)
  const air = await goods(page.request, airCat.id, `Фильтр A1 ${RUN}`, 60_000, 20_000)
  const party = await api<{ id: string }>(page.request, 'POST', '/parties', { name: `Фильтров ${RUN}`, kind: 'person' })
  const plate = `01KG${RUN.slice(-3)}FFF`
  const vehicle = await api<{ id: string }>(page.request, 'POST', `/parties/${party.id}/vehicles`, { plate })
  await api(page.request, 'POST', '/sales', {
    op_id: crypto.randomUUID(),
    sale_type: 'service',
    cashier_id: emp.id,
    master_id: mst.id,
    party_id: party.id,
    vehicle_id: vehicle.id,
    mileage_km: 85_000,
    lines: [
      { kind: 'container', gift: false, product_id: oil.id, qty: 1, unit_price_tyiyn: 700_000 },
      { kind: 'piece', gift: false, product_id: filter.id, qty: 1, unit_price_tyiyn: 50_000 },
      { kind: 'piece', gift: false, product_id: air.id, qty: 1, unit_price_tyiyn: 60_000 },
    ],
    payments: [{ method: 'cash', amount_tyiyn: 810_000 }],
  })

  // Карточка клиента: три строки, у каждой своя следующая замена.
  await page.goto(`/clients/${party.id}`)
  const slots = page.getByTestId('book-slots')
  await expect(slots.getByRole('row', { name: /Масло двигателя/ })).toContainText(/на 93\s000 км/)
  await expect(slots.getByRole('row', { name: /Фильтр масляный/ })).toContainText(/на 93\s000 км/)
  const airRow = slots.getByRole('row', { name: /Фильтр воздушный/ })
  await expect(airRow).toContainText(/на 105\s000 км/)
  await expect(page.getByText(`Фильтр A1 ${RUN} · 1 шт`)).toBeVisible()

  // Свой интервал воздушного фильтра у этой машины.
  await airRow.getByRole('button', { name: 'Интервал' }).click()
  await page.getByLabel('Каждые, км').fill('10000')
  await page.getByRole('button', { name: 'Сохранить' }).click()
  await expect(airRow).toContainText(/на 95\s000 км/)
  await expect(airRow).toContainText('у этой машины')
  await expect(slots.getByRole('row', { name: /Масло двигателя/ })).toContainText(/на 93\s000 км/)

  // Печать: строки последней замены и следующая по каждой строке.
  await page.getByRole('button', { name: 'Печать книжки' }).click()
  await expect.poll(async () => (await prints(page)).some((h) => h.includes('Масляная книжка'))).toBe(true)
  const html = (await prints(page)).find((h) => h.includes('Масляная книжка')) ?? ''
  expect(html).toContain('Фильтр воздушный')
  expect(html).toContain(`Фильтр A1 ${RUN}`)
  expect(html).toMatch(/на 95\s000 км/)

  // Касса: под выбранной машиной — следующая замена по строкам.
  await page.goto('/')
  // Клиента находят по госномеру — машина подставляется сама.
  await page.locator('[data-client-input]').fill(plate)
  await page.getByRole('button', { name: new RegExp(`Фильтров ${RUN}`) }).first().click()
  const hint = page.getByTestId('oil-hint')
  await expect(hint).toContainText(/Фильтр воздушный: следующая на 95\s000 км/)
})
