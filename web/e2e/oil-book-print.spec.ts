// Масляная книжка на выводе (задача 003 Н5, ADR-144): чек без цели кассира — сервер ставит пробег
// + интервал, и товарный чек, и чек книжки показывают одно число; канистра и разлив одного масла —
// одной строкой «4,5 л»; «Печать книжки» — в экране чека; после «Исправить» книжка нового чека
// печатается сама; «Настройки → Масляная книжка и печать» убирает блок книжки из товарного чека.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ownerSafe } from './safe'
import { choosePerson, ensureShift, pickCashier, pickMaster, shiftOnTill } from './cashier'

/** Части по порядку, между ними что угодно (чек книжки — три наклейки, задача 009). */
const inOrder = (...parts: string[]) => new RegExp(parts.map((p) => p.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('[\\s\\S]*'))

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000eb03' }
const RUN = Date.now().toString().slice(-8)

/** Печать идёт через скрытый фрейм: HTML каждого документа — в window.__prints. */
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

const prints = async (page: Page) => (await page.evaluate(() => (window as unknown as { __prints?: string[] }).__prints ?? [])).map((h) => h.replace(/&nbsp;/g, ' '))
const plain = (html: string) => html.replace(/<style[\s\S]*?<\/style>/g, '').replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ')
const lastPrint = async (page: Page, what: string) => plain((await prints(page)).filter((h) => h.includes(what)).at(-1) ?? '')

async function api<T>(request: APIRequestContext, method: 'POST' | 'PUT' | 'GET', path: string, data?: unknown): Promise<T> {
  const res = await request.fetch(`/api/v1${path}`, { method, data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

/** Чек «В сервис» через API, как его прислало бы старое устройство: без следующей замены. */
async function serviceSale(request: APIRequestContext) {
  const { body, oilName, vehicle } = await serviceBody(request, RUN)
  // Онлайн-чек — только при открытой смене (задача 005 Н4).
  await ensureShift(request, body.cashier_id)
  const sale = await api<{ id: string; number: number; vehicle_id: string }>(request, 'POST', '/sales', body)
  expect(sale.vehicle_id).toBe(vehicle)
  return { sale, oilName }
}

/** Товары, мастер, кассир, клиент с машиной и тело чека «В сервис» (оплата картой). */
async function serviceBody(request: APIRequestContext, tag: string) {
  const safe = await ownerSafe(request)
  const oilCat = await api<{ id: string }>(request, 'POST', '/categories', { name: `Масла книжки ${tag}`, kind: 'oil' })
  const oilName = `Масло книжки ${tag} 5W-30`
  const oil = await api<{ id: string }>(request, 'POST', '/products', { op_id: crypto.randomUUID(), category_id: oilCat.id, name: oilName, container_ml: 4000, sale_price_tyiyn: 300_000 })
  await api(request, 'POST', '/receipts', { op_id: crypto.randomUUID(), paid_from: safe, lines: [{ product_id: oil.id, qty: 40_000, cost_tyiyn: 2_000_000 }] })
  const svc = await api<{ id: string; price_tyiyn: number }>(request, 'POST', '/services', { name: `Замена масла в моторе книжки ${tag}`, price_tyiyn: 20_000, master_fee_tyiyn: 3_000 })
  const mst = await api<{ id: string }>(request, 'POST', '/employees', { full_name: `Мастер книжки ${tag}`, is_master: true })
  const csh = await api<{ id: string }>(request, 'POST', '/employees', { full_name: `Кассир книжки ${tag}`, is_cashier: true, pin: '1234' })
  const party = await api<{ id: string }>(request, 'POST', '/parties', { kind: 'person', first_name: 'Азиз', last_name: `Книжка${tag}`, phone: `0702${tag.slice(-6)}` })
  const vehicle = await api<{ id: string }>(request, 'POST', `/parties/${party.id}/vehicles`, { plate: `09KG${tag.slice(-3)}BKK`, brand: 'Toyota', model: 'Camry' })
  const total = 300_000 + 37_500 + svc.price_tyiyn
  // Картой: «Исправить» такого чека не требует открытой смены.
  const body = {
    op_id: crypto.randomUUID(),
    client_time: new Date().toISOString(),
    sale_type: 'service',
    cashier_id: csh.id,
    master_id: mst.id,
    party_id: party.id,
    vehicle_id: vehicle.id,
    mileage: 130_000,
    mileage_unit: 'km',
    lines: [
      { kind: 'container', gift: false, product_id: oil.id, qty: 1, unit_price_tyiyn: 300_000 },
      { kind: 'pour', gift: false, product_id: oil.id, qty: 500, unit_price_tyiyn: 75_000 },
      { kind: 'service', gift: false, service_id: svc.id, qty: 1, unit_price_tyiyn: svc.price_tyiyn },
    ],
    payments: [{ method: 'card', amount_tyiyn: total }],
  }
  return { body, oilName, vehicle: vehicle.id }
}

/** Кладёт чек в очередь устройства, как касса без сети, и будит отправку (событие «online»). */
async function queueOffline(page: Page, userId: string, body: Record<string, unknown>, tempNo: string) {
  await page.evaluate(
    async ({ uid, body, tempNo }) => {
      const db = await new Promise<IDBDatabase>((resolve, reject) => {
        const r = indexedDB.open('avtodom', 1)
        r.onsuccess = () => resolve(r.result)
        r.onerror = () => reject(r.error)
      })
      await new Promise<void>((resolve, reject) => {
        const tx = db.transaction('outbox', 'readwrite')
        tx.objectStore('outbox').put({
          op_id: body.op_id,
          temp_no: tempNo,
          created_at: new Date(Date.now() - 60_000).toISOString(),
          status: 'pending',
          attempts: 1,
          last_error: 'нет связи',
          user_id: uid,
          user_name: 'Владелец',
          body,
        })
        tx.oncomplete = () => resolve()
        tx.onerror = () => reject(tx.error)
      })
      window.dispatchEvent(new Event('online'))
    },
    { uid: userId, body, tempNo },
  )
}

test('чек «В сервис» без сети: книжка печатается сама после отправки, выключено — кнопкой в уведомлении', async ({ page }) => {
  await page.addInitScript(PRINT_STUB)
  await login(page)
  const initial = await api<Record<string, unknown>>(page.request, 'GET', '/settings/oil-book')
  expect(initial.offline_book).toBe(true)
  const me = await api<{ id: string }>(page.request, 'GET', '/auth/me')
  try {
    await page.goto('/')
    await page.waitForSelector('[data-picker]')
    const first = await serviceBody(page.request, `${RUN}o1`)
    await queueOffline(page, me.id, { ...first.body, client_time: new Date(Date.now() - 60_000).toISOString() }, 'Ч-OFF-0001')
    // Сервер принял чек — чек книжки ушёл на печать сам.
    await expect.poll(async () => (await prints(page)).some((h) => h.includes('Масляная книжка') && h.includes(first.oilName)), { timeout: 20_000 }).toBe(true)
    await expect(page.getByTestId('offline-book-prints')).toHaveCount(0)

    // Выключено — книжка не печатается, в уведомлении кнопка «Печать книжки».
    await api(page.request, 'PUT', '/settings/oil-book', { ...initial, offline_book: false, op_id: crypto.randomUUID() })
    const second = await serviceBody(page.request, `${RUN}o2`)
    const before = (await prints(page)).length
    await queueOffline(page, me.id, { ...second.body, client_time: new Date(Date.now() - 60_000).toISOString() }, 'Ч-OFF-0002')
    const notice = page.getByTestId('offline-book-prints')
    await expect(notice).toContainText('Книжка не напечатана', { timeout: 20_000 })
    expect((await prints(page)).slice(before).some((h) => h.includes(second.oilName))).toBe(false)
    await notice.getByRole('button', { name: 'Печать книжки' }).click()
    await expect.poll(async () => (await prints(page)).slice(before).some((h) => h.includes('Масляная книжка') && h.includes(second.oilName))).toBe(true)
    await expect(notice).toHaveCount(0)
  } finally {
    await api(page.request, 'PUT', '/settings/oil-book', { ...initial, op_id: crypto.randomUUID() })
  }
})

test('чек без цели кассира: «Печать книжки» в экране чека, одно число и «4,5 л» одной строкой; «Исправить» печатает книжку', async ({ page }) => {
  await page.addInitScript(PRINT_STUB)
  await login(page)
  const { sale, oilName } = await serviceSale(page.request)

  await page.goto(`/sales/${sale.id}`)
  // Товарный чек: блок книжки со следующей заменой, которую поставил сервер.
  await page.getByRole('button', { name: 'Печать', exact: true }).click()
  await expect.poll(async () => (await prints(page)).some((h) => h.includes('Товарный чек'))).toBe(true)
  const receipt = await lastPrint(page, 'Товарный чек')
  // Подпись — товар, а не «Масло двигателя» (задача 003 Н5).
  expect(receipt).toContain(`${oilName} · 4,5 л Следующая замена — на 138 000 км`)
  expect(receipt).not.toContain('Масло двигателя')
  // Чек книжки этого чека — то же число, масло одной строкой.
  await page.getByRole('button', { name: 'Печать книжки' }).click()
  await expect.poll(async () => (await prints(page)).filter((h) => h.includes('Масляная книжка') && !h.includes('Товарный чек')).length).toBe(1)
  const book = plain((await prints(page)).find((h) => h.includes('Масляная книжка') && !h.includes('Товарный чек')) ?? '')
  expect(book).toContain(`${oilName} · 4,5 л`)
  expect(book).toMatch(inOrder(`${oilName} · 4,5 л`, 'Следующая замена', '— на 138 000 км'))
  expect(book).not.toContain('Масло двигателя')

  // «Исправить»: новый чек печатает книжку сам, как после обычного чека.
  await page.getByTestId('fix-button').click()
  await expect(page.getByTestId('correction-banner')).toContainText(`№ ${sale.number}`)
  await page.locator('#correction-comment').fill('проверка книжки после исправления')
  const before = (await prints(page)).length
  await page.getByRole('button', { name: 'Сохранить исправление' }).click()
  await expect(page).toHaveURL(/\/sales\/[0-9a-f-]+$/)
  await expect(page).not.toHaveURL(new RegExp(sale.id))
  await expect.poll(async () => (await prints(page)).slice(before).some((h) => h.includes('Масляная книжка') && !h.includes('Товарный чек'))).toBe(true)
  await expect(page.getByRole('button', { name: 'Печать книжки' })).toBeVisible()
})

test('«Масляная книжка и печать»: без блока книжки в товарном чеке, изменение сохраняется', async ({ page }) => {
  await page.addInitScript(PRINT_STUB)
  await login(page)
  const { sale } = await serviceSale(page.request)
  const initial = await api<Record<string, unknown>>(page.request, 'GET', '/settings/oil-book')
  try {
    await page.goto('/settings')
    await page.getByRole('link', { name: /Масляная книжка и печать/ }).click()
    const block = page.getByTestId('oil-book-receipt_block')
    await expect(block).toBeChecked()
    await block.uncheck()
    await expect(page.getByText('В товарном чеке — без блока книжки')).toBeVisible()
    await page.reload()
    await expect(page.getByTestId('oil-book-receipt_block')).not.toBeChecked()
    await expect(page.getByTestId('oil-book-auto_book')).toBeChecked()

    await page.goto(`/sales/${sale.id}`)
    await expect(page.getByRole('button', { name: 'Печать книжки' })).toBeVisible()
    await page.getByRole('button', { name: 'Печать', exact: true }).click()
    await expect.poll(async () => (await prints(page)).some((h) => h.includes('Товарный чек'))).toBe(true)
    expect(await lastPrint(page, 'Товарный чек')).not.toContain('Масляная книжка')
  } finally {
    await api(page.request, 'PUT', '/settings/oil-book', { ...initial, op_id: crypto.randomUUID() })
  }
})

test('«В сервис» с маслом и фильтрами: чек книжки печатается сам — товары по виду замены и следующая у каждого', async ({ page }) => {
  // Задача 003 Н5 (31.10.2026): «если в сервис, то он должен печатать, и печатать то, что было указано в
  // вид замены»; «не "масло в коробке", а какое масло»; топливный фильтр — свой срок.
  test.setTimeout(90_000)
  await page.addInitScript(PRINT_STUB)
  await login(page)
  const request = page.request
  const tag = `${RUN}f`
  const safe = await ownerSafe(request)
  // По умолчанию: чек книжки сам и с фильтрами.
  const prefs = await api<{ auto_book: boolean; with_filters: boolean }>(request, 'GET', '/settings/oil-book')
  expect([prefs.auto_book, prefs.with_filters]).toEqual([true, true])
  const oilCat = await api<{ id: string }>(request, 'POST', '/categories', { name: `Масла фильтров ${tag}`, kind: 'oil' })
  const filterCat = await api<{ id: string }>(request, 'POST', '/categories', { name: `Фильтры вида ${tag}`, kind: 'filter' })
  // Топливные — своя категория со своим интервалом; «Тип» у товара не указан — строка от категории.
  const fuelCat = await api<{ id: string }>(request, 'POST', '/categories', { name: `Топливные ${tag}`, kind: 'filter', book: { slot: 'fuel_filter', interval_km: 40_000, interval_days: 730 } })
  const goods = [
    { key: 'oil', cat: oilCat.id, name: `Mobil 1 ${tag} 5W-30`, extra: { container_ml: 4000 }, qty: 40_000 },
    { key: 'oilf', cat: filterCat.id, name: `W712-${tag}`, extra: { attrs: { filter_type: 'масляный' } }, qty: 5 },
    { key: 'air', cat: filterCat.id, name: `C2201-${tag}`, extra: { attrs: { filter_type: 'воздушный' } }, qty: 5 },
    { key: 'cab', cat: filterCat.id, name: `CU2939-${tag}`, extra: { attrs: { filter_type: 'салонный' } }, qty: 5 },
    { key: 'fuel', cat: fuelCat.id, name: `Фильтр топливный WK820-${tag}`, extra: {}, qty: 5 },
  ]
  const codes: string[] = []
  const lines = []
  for (const [i, g] of goods.entries()) {
    const code = `48${tag.slice(0, 8)}${i}`
    codes.push(code)
    const p = await api<{ id: string }>(request, 'POST', '/products', { op_id: crypto.randomUUID(), category_id: g.cat, name: g.name, barcodes: [code], sale_price_tyiyn: 50_000, ...g.extra })
    lines.push({ product_id: p.id, qty: g.qty, cost_tyiyn: 100_000 })
  }
  await api(request, 'POST', '/receipts', { op_id: crypto.randomUUID(), paid_from: safe, lines })
  const service = `Замена масла в моторе вида ${tag}`
  await api(request, 'POST', '/services', { name: service, price_tyiyn: 20_000, master_fee_tyiyn: 3_000 })
  const master = `Мастер вида ${tag}`
  await api(request, 'POST', '/employees', { full_name: master, is_master: true })
  const cashier = `Кассир вида ${tag}`
  await api(request, 'POST', '/employees', { full_name: cashier, is_cashier: true, pin: '1234' })
  const last = `Вид${tag}`
  const party = await api<{ id: string }>(request, 'POST', '/parties', { kind: 'person', first_name: 'Азиз', last_name: last, phone: `0703${tag.slice(-7, -1)}` })
  const plate = `07KG${tag.slice(-4, -1)}VID`
  await api(request, 'POST', `/parties/${party.id}/vehicles`, { plate, brand: 'Toyota', model: 'Camry' })

  await page.goto('/')
  await expect(page.locator('[data-picker]')).toBeVisible()
  await page.getByRole('button', { name: 'В сервис' }).click()
  const kinds = page.getByRole('dialog', { name: 'Вид замены' })
  await kinds.getByPlaceholder('Найти услугу').fill(service)
  await kinds.getByTestId('service-choices').getByRole('button', { name: new RegExp(service) }).click()
  await kinds.getByRole('button', { name: 'Готово' }).click()
  const cart = page.getByTestId('cart').getByRole('listitem')
  for (const [i, code] of codes.entries()) {
    await page.locator('[data-picker]').focus()
    await page.keyboard.type(code, { delay: 5 })
    await page.keyboard.press('Enter')
    await expect(cart).toHaveCount(i + 2)
  }
  await shiftOnTill(page, cashier)
  if (await page.locator('#cashier-select').isVisible()) await choosePerson(page, 'cashier-select', cashier)
  await pickMaster(page, master)
  await page.locator('[data-client-input]').fill(plate)
  await page.getByRole('button', { name: new RegExp(last) }).first().click()
  await page.locator('#mileage').fill('130000')
  // Фильтры с маслом своих строк не дают — они под строкой масла.
  await expect(page.getByTestId('next-filters')).toContainText(`W712-${tag} (масляный), C2201-${tag} (воздушный), CU2939-${tag} (салонный) на тот же пробег`)
  await page.getByRole('button', { name: 'Без сдачи' }).click()
  const before = (await prints(page)).length
  await page.getByRole('button', { name: /Провести чек/ }).first().click()
  await expect(page.getByText(/Чек № .+ проведён/)).toBeVisible()

  // Чек книжки — сам, без кнопки.
  await expect.poll(async () => (await prints(page)).slice(before).some((h) => h.includes('Масляная книжка') && !h.includes('Товарный чек'))).toBe(true)
  const book = plain((await prints(page)).slice(before).find((h) => h.includes('Масляная книжка') && !h.includes('Товарный чек')) ?? '')
  expect(book).toMatch(
    inOrder(service, `Mobil 1 ${tag} 5W-30 · 1 кан. × 4 л`, `W712-${tag} (масляный) · 1 шт`, `C2201-${tag} (воздушный) · 1 шт`, `CU2939-${tag} (салонный) · 1 шт`, 'Следующая замена', '— на 138 000 км'),
  )
  // Топливный — свой срок (интервал его категории), не пробег масла.
  expect(book).toMatch(inOrder(`Фильтр топливный WK820-${tag} · 1 шт`, 'Следующая замена', '— на 170 000 км'))
  for (const no of ['Масло двигателя', 'Фильтр масляный', 'Фильтр воздушный', 'Фильтр салонный', 'Масло в коробке']) expect(book).not.toContain(no)
  // Товарный чек — тот же блок.
  await page.getByRole('button', { name: 'Печать чека' }).click()
  await expect.poll(async () => (await lastPrint(page, 'Товарный чек')).length).toBeGreaterThan(0)
  const receipt = await lastPrint(page, 'Товарный чек')
  expect(receipt).toContain(`CU2939-${tag} (салонный) · 1 шт Следующая замена — на 138 000 км`)
  expect(receipt).toContain(`Фильтр топливный WK820-${tag} · 1 шт Следующая замена — на 170 000 км`)
})

test('чек без сети и две вкладки кассы: чек уходит один раз, книжка печатается одна, уведомления нет ни в одной', async ({ page, context }) => {
  await context.addInitScript(PRINT_STUB)
  await login(page)
  const me = await api<{ id: string }>(page.request, 'GET', '/auth/me')
  await page.goto('/')
  await page.waitForSelector('[data-picker]')
  const sale = await serviceBody(page.request, `${RUN}t2`)
  const body = { ...sale.body, client_time: new Date(Date.now() - 60_000).toISOString() }
  // Ответ на чек из очереди идёт долго: пока первая вкладка его ждёт, открывают вторую.
  let posts = 0
  await context.route('**/api/v1/sales', async (route) => {
    if (route.request().method() !== 'POST') return route.continue()
    posts += 1
    await new Promise((r) => setTimeout(r, 3000))
    await route.continue()
  })
  await queueOffline(page, me.id, body, 'Ч-OFF-0003')
  await expect
    .poll(() =>
      page.evaluate(async (op) => {
        const db = await new Promise<IDBDatabase>((resolve, reject) => {
          const r = indexedDB.open('avtodom', 1)
          r.onsuccess = () => resolve(r.result)
          r.onerror = () => reject(r.error)
        })
        return new Promise<string>((resolve) => {
          const g = db.transaction('outbox').objectStore('outbox').get(op)
          g.onsuccess = () => resolve((g.result as { status?: string } | undefined)?.status ?? '')
        })
      }, String(body.op_id)),
    )
    .toBe('sending')
  const other = await context.newPage()
  await other.goto('/')
  await other.waitForSelector('[data-picker]')
  await other.evaluate(() => window.dispatchEvent(new Event('online')))
  const books = async () => {
    let n = 0
    for (const p of [page, other]) n += (await prints(p)).filter((h) => h.includes('Масляная книжка') && h.includes(sale.oilName)).length
    return n
  }
  await expect.poll(books, { timeout: 20_000 }).toBe(1)
  // Вторая вкладка чек не отправила и книжку второй раз не печатает; уведомления нет ни в одной.
  await page.waitForTimeout(4000)
  expect(await books()).toBe(1)
  expect(posts).toBe(1)
  await expect(page.getByTestId('offline-book-prints')).toHaveCount(0)
  await expect(other.getByTestId('offline-book-prints')).toHaveCount(0)
  await context.unroute('**/api/v1/sales')
  const sent = await api<{ number: number }>(page.request, 'GET', `/sales/by-op/${String(body.op_id)}`)
  expect(sent.number).toBeGreaterThan(0)
  await other.close()
})

test('«Товарный чек после любого чека»: чек «На вынос» печатается сам один раз, выключено — нет (задача 023)', async ({ page }) => {
  await page.addInitScript(PRINT_STUB)
  await login(page)
  const tag = `${RUN}23`
  const cashier = `Кассир автопечати ${tag}`
  await api(page.request, 'POST', '/employees', { full_name: cashier, is_cashier: true, pin: '1234' })
  const cat = await api<{ id: string }>(page.request, 'POST', '/categories', { name: `Автопечать ${tag}`, kind: 'other' })
  const code = `23${tag}`
  const item = `Ветошь автопечати ${tag}`
  const p = await api<{ id: string }>(page.request, 'POST', '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name: item, sale_price_tyiyn: 10_000, barcodes: [code] })
  await api(page.request, 'POST', '/receipts', { op_id: crypto.randomUUID(), paid_from: await ownerSafe(page.request), lines: [{ product_id: p.id, qty: 5, cost_tyiyn: 5_000 }] })
  const initial = await api<Record<string, unknown>>(page.request, 'GET', '/settings/oil-book')
  const sell = async () => {
    await page.goto('/')
    await pickCashier(page, cashier)
    const picker = page.getByPlaceholder(/Сканируйте штрихкод/)
    await picker.fill(code)
    await picker.press('Enter')
    await expect(page.getByText(item)).toBeVisible()
    await page.getByRole('button', { name: 'Провести чек' }).click()
    await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()
  }
  const receipts = async () => (await prints(page)).filter((h) => h.includes('Товарный чек') && h.includes(item)).length
  try {
    await api(page.request, 'PUT', '/settings/oil-book', { ...initial, auto_receipt: false, auto_receipt_all: true, op_id: crypto.randomUUID() })
    await sell()
    await expect.poll(receipts).toBe(1)
    await page.waitForTimeout(500)
    expect(await receipts()).toBe(1)

    await api(page.request, 'PUT', '/settings/oil-book', { ...initial, auto_receipt: false, auto_receipt_all: false, op_id: crypto.randomUUID() })
    await sell()
    await page.waitForTimeout(500)
    expect(await receipts()).toBe(0)
  } finally {
    await api(page.request, 'PUT', '/settings/oil-book', { ...initial, op_id: crypto.randomUUID() })
  }
})

test('QR — своя наклейка своей печатью после книжки; «Печать QR»; выключено — только книжка (задача 024)', async ({ page }) => {
  test.setTimeout(90_000)
  await page.addInitScript(PRINT_STUB)
  // Бот подключён — у кассы есть ссылка для QR.
  await page.route('**/api/v1/telegram/bot', (r) => r.fulfill({ json: { username: 'avtodom_test_bot' } }))
  await login(page)
  const { body, oilName } = await serviceBody(page.request, `${RUN}q`)
  await ensureShift(page.request, body.cashier_id)
  const sale = await api<{ id: string; number: number }>(page.request, 'POST', '/sales', body)
  const initial = await api<Record<string, unknown>>(page.request, 'GET', '/settings/oil-book')
  expect(initial.qr_sticker).toBe(true)
  const isQr = (h: string) => h.includes('QR Телеграм-бота')
  try {
    await page.goto(`/sales/${sale.id}`)
    // «Печать книжки» — только книжка, без QR.
    await page.getByRole('button', { name: 'Печать книжки' }).click()
    await expect.poll(async () => (await prints(page)).length).toBe(1)
    const book = (await prints(page))[0]
    expect(book).toContain(oilName)
    expect(book).not.toContain('<svg')
    expect(isQr(book)).toBe(false)
    // «Печать QR» — одна наклейка 58×40 с QR.
    await page.getByRole('button', { name: 'Печать QR' }).click()
    await expect.poll(async () => (await prints(page)).length).toBe(2)
    const qr = (await prints(page))[1]
    expect(isQr(qr)).toBe(true)
    expect(qr).toContain('size: 58mm 40mm')
    expect(qr.match(/<svg/g)).toHaveLength(1)
    expect(plain(qr)).toContain('Масляная книжка в Телеграм')

    // «Исправить» — печать как после чека: два задания, сначала книжка, затем QR.
    const fix = async (comment: string) => {
      await page.getByTestId('fix-button').click()
      await page.locator('#correction-comment').fill(comment)
      const url = page.url()
      await page.getByRole('button', { name: 'Сохранить исправление' }).click()
      await expect(page).not.toHaveURL(url)
      await expect(page.getByRole('button', { name: 'Печать книжки' })).toBeVisible()
    }
    let before = (await prints(page)).length
    await fix('проверка QR после исправления')
    await expect.poll(async () => (await prints(page)).length - before).toBe(2)
    const [b2, q2] = (await prints(page)).slice(before)
    expect(b2).toContain(oilName)
    expect(isQr(b2)).toBe(false)
    expect(isQr(q2)).toBe(true)

    // Выключено — после чека только книжка; QR — кнопкой.
    await api(page.request, 'PUT', '/settings/oil-book', { ...initial, qr_sticker: false, op_id: crypto.randomUUID() })
    await page.reload()
    before = (await prints(page)).length
    await fix('проверка без QR')
    await expect.poll(async () => (await prints(page)).slice(before).some((h) => h.includes(oilName))).toBe(true)
    await page.waitForTimeout(1_000)
    expect((await prints(page)).slice(before).some(isQr)).toBe(false)
  } finally {
    await api(page.request, 'PUT', '/settings/oil-book', { ...initial, op_id: crypto.randomUUID() })
  }
})

test('станция книжки: книжка уходит на второе окно со своим принтером; станции нет — печать здесь (задача 027)', async ({ page, context }) => {
  await context.addInitScript(PRINT_STUB)
  await login(page)
  const { sale, oilName } = await serviceSale(page.request)
  const initial = await api<Record<string, unknown>>(page.request, 'GET', '/settings/oil-book')
  const isBook = (h: string) => h.includes('Масляная книжка') && !h.includes('Товарный чек')
  try {
    await api(page.request, 'PUT', '/settings/oil-book', { ...initial, book_station: true, op_id: crypto.randomUUID() })
    // Станции ещё нет — книжка печатается здесь, касса говорит почему.
    await page.goto(`/sales/${sale.id}`)
    await page.getByRole('button', { name: 'Печать книжки' }).click()
    await expect(page.getByText('Станция книжки не в сети — книжка печатается здесь')).toBeVisible()
    await expect.poll(async () => (await prints(page)).filter(isBook).length).toBe(1)

    // Окно станции книжки открыто — книжка печатается в нём, а не на кассе.
    const station = await context.newPage()
    await station.goto('/label-station?size=book')
    await expect(station.getByText('Принтер: книжка — готов')).toBeVisible()
    await page.reload()
    await page.getByRole('button', { name: 'Печать книжки' }).click()
    await expect(page.getByText('Книжка ушла на принтер книжки')).toBeVisible()
    await expect.poll(async () => (await prints(station)).filter(isBook).length, { timeout: 15_000 }).toBe(1)
    expect(plain((await prints(station)).find(isBook) ?? '')).toContain(`${oilName} · 4,5 л`)
    await expect(station.getByLabel('Задания печати')).toContainText('напечатано')
    expect((await prints(page)).filter(isBook)).toHaveLength(0)
  } finally {
    await api(page.request, 'PUT', '/settings/oil-book', { ...initial, op_id: crypto.randomUUID() })
  }
})
