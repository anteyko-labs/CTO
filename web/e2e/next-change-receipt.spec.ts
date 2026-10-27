// Итог чека «В сервис» и книжка, которая печатается после него: «Следующая замена» — только по
// строкам этого чека (ADR-111). 26.10.2026 заказчик: «в чеке постоянно добавляет воздушный фильтр» —
// после каждого чека машины касса писала «Следующая · Фильтр воздушный» из прошлой замены.
import { expect, test, type APIRequestContext, type Locator, type Page } from '@playwright/test'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e354' }
const RUN = Date.now().toString().slice(-8)
// Снимки экрана для проверки раскладки: только если задан каталог.
const SHOTS = process.env.E2E_SHOTS_DIR
let seq = 0

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

async function api<T>(request: APIRequestContext, path: string, data: unknown): Promise<T> {
  const res = await request.post(`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

/** Машина с прошлой заменой 17.10 «масло + воздушный фильтр» на 216 000 км. */
async function setup(request: APIRequestContext) {
  const tag = `${RUN}${++seq}`
  const safe = await ownerSafe(request)
  const oilCat = await api<{ id: string }>(request, '/categories', { name: `Масла итог ${tag}`, kind: 'oil' })
  const airCat = await api<{ id: string }>(request, '/categories', { name: `Фильтры итог ${tag}`, kind: 'filter', book: { slot: 'air_filter', interval_km: 15_000, interval_days: 365 } })
  const oilCode = `45${tag}1`
  const oil = await api<{ id: string }>(request, '/products', { op_id: crypto.randomUUID(), category_id: oilCat.id, name: `Масло итог ${tag} 5W-30`, container_ml: 4000, barcodes: [oilCode], sale_price_tyiyn: 300_000 })
  const air = await api<{ id: string }>(request, '/products', { op_id: crypto.randomUUID(), category_id: airCat.id, name: `Фильтр воздушный итог ${tag}`, barcodes: [`45${tag}2`], sale_price_tyiyn: 60_000 })
  await api(request, '/receipts', {
    op_id: crypto.randomUUID(),
    paid_from: safe,
    lines: [
      { product_id: oil.id, qty: 40_000, cost_tyiyn: 2_000_000 },
      { product_id: air.id, qty: 10, cost_tyiyn: 300_000 },
    ],
  })
  const service = `Замена масла в моторе итог ${tag}`
  const svc = await api<{ id: string; price_tyiyn: number }>(request, '/services', { name: service, price_tyiyn: 20_000, master_fee_tyiyn: 3_000 })
  const master = `Мастер итог ${tag}`
  const mst = await api<{ id: string }>(request, '/employees', { full_name: master, is_master: true })
  const cashier = `Кассир итог ${tag}`
  const csh = await api<{ id: string }>(request, '/employees', { full_name: cashier, is_cashier: true, pin: '1234' })
  const last = `Итог${tag}`
  const party = await api<{ id: string }>(request, '/parties', { kind: 'person', first_name: 'Азиз', last_name: last, phone: `0701${tag.slice(-6)}` })
  const plate = `09KG${tag.slice(-3)}ZXZ`
  const vehicle = await api<{ id: string }>(request, `/parties/${party.id}/vehicles`, { plate, brand: 'Toyota', model: 'Camry' })
  // Прошлая замена: масло, воздушный фильтр и услуга.
  await api(request, '/sales', {
    op_id: crypto.randomUUID(),
    client_time: new Date().toISOString(),
    sale_type: 'service',
    cashier_id: csh.id,
    master_id: mst.id,
    party_id: party.id,
    vehicle_id: vehicle.id,
    mileage: 216_000,
    mileage_unit: 'km',
    lines: [
      { kind: 'container', gift: false, product_id: oil.id, qty: 1, unit_price_tyiyn: 300_000 },
      { kind: 'piece', gift: false, product_id: air.id, qty: 1, unit_price_tyiyn: 60_000 },
      { kind: 'service', gift: false, service_id: svc.id, qty: 1, unit_price_tyiyn: svc.price_tyiyn },
    ],
    payments: [{ method: 'cash', amount_tyiyn: 300_000 + 60_000 + svc.price_tyiyn }],
  })
  return { oilCode, service, master, cashier, last, plate, party }
}

for (const mode of [
  { title: 'обычная касса', touch: false },
  { title: 'режим кассы 1366×768', touch: true },
] as const) {
  test.describe(mode.title, () => {
    test.use({ viewport: { width: 1366, height: 768 }, hasTouch: mode.touch })
    const hit = (l: Locator) => (mode.touch ? l.tap() : l.click())

    test('итог чека и книжка после него: следующая замена только по строкам этого чека', async ({ page }) => {
      await page.addInitScript(PRINT_STUB)
      await page.addInitScript((touch) => {
        if (touch) localStorage.setItem('avtodom.cashier_mode', '1')
        else localStorage.removeItem('avtodom.cashier_mode')
      }, mode.touch)
      await page.goto('/login')
      await page.getByLabel('Логин').fill(OWNER)
      await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
      await page.getByRole('button', { name: 'Войти' }).click()
      await expect(page.locator('[data-picker]')).toBeVisible()
      const s = await setup(page.request)
      await page.reload()
      await expect(page.locator('[data-picker]')).toBeFocused()

      // Чек «В сервис»: только замена масла и масло — фильтр в этот раз не меняли.
      await hit(page.getByRole('button', { name: 'В сервис' }))
      const kinds = page.getByRole('dialog', { name: 'Вид замены' })
      await kinds.getByPlaceholder('Найти услугу').fill(s.service)
      await hit(kinds.getByTestId('service-choices').getByRole('button', { name: new RegExp(s.service) }))
      await hit(kinds.getByRole('button', { name: 'Готово' }))
      await page.locator('[data-picker]').focus()
      await page.keyboard.type(s.oilCode, { delay: 5 })
      await page.keyboard.press('Enter')
      await expect(page.getByTestId('cart').getByRole('listitem')).toHaveCount(2)
      const select = page.locator('#cashier-select')
      if (await select.isVisible()) await select.selectOption({ label: s.cashier })
      await page.locator('#master-select').selectOption({ label: s.master })
      if (mode.touch) {
        await hit(page.getByTestId('service-left').getByRole('button', { name: 'Выбрать клиента' }))
        const dlg = page.getByRole('dialog', { name: 'Клиент и машина' })
        await dlg.locator('[data-client-input]').fill(s.plate)
        await hit(dlg.getByRole('button', { name: new RegExp(s.last) }).first())
        await hit(dlg.getByRole('button', { name: 'Готово' }))
      } else {
        await page.locator('[data-client-input]').fill(s.plate)
        await page.getByRole('button', { name: new RegExp(s.last) }).first().click()
      }
      await page.locator('#mileage').fill('220000')
      await hit(page.getByRole('button', { name: 'Без сдачи' }))
      await hit(page.getByRole('button', { name: /Провести чек/ }).first())
      await expect(page.getByText(/Чек № .+ проведён/)).toBeVisible()

      // Итог чека: что записано в книжку — масло; воздушного фильтра нет.
      const written = page.getByText(/Записано в масляную книжку/).locator('..')
      await expect(written).toContainText('Следующая · Масло двигателя')
      await expect(written).not.toContainText('Фильтр воздушный')

      // Книжка, напечатанная сама: «Следующая замена» — без фильтра; он — по прошлым заменам.
      await expect.poll(async () => (await prints(page)).some((h) => h.includes('Масляная книжка'))).toBe(true)
      const bookHtml = (await prints(page)).find((h) => h.includes('Масляная книжка')) ?? ''
      const book = plain(bookHtml)
      if (SHOTS && !mode.touch) {
        await page.screenshot({ path: `${SHOTS}/next-change-receipt-card.png` })
        const view = await page.context().newPage()
        await view.setViewportSize({ width: 320, height: 700 })
        await view.setContent(bookHtml)
        await view.screenshot({ path: `${SHOTS}/next-change-receipt-book.png`, fullPage: true })
        await view.close()
      }
      const next = book.split('Следующая замена — что наступит раньше')[1]?.split(/По прошлым заменам|Прошлые замены/)[0] ?? ''
      expect(next).toContain('Масло двигателя')
      expect(next).not.toContain('Фильтр воздушный')
      expect(book).toMatch(/По прошлым заменам.*Фильтр воздушный \(замена \d\d\.\d\d\.\d{4}\)/)

      // Товарный чек — без фильтра; в карточке клиента строка фильтра на месте.
      await hit(page.getByRole('button', { name: 'Печать чека' }))
      await expect.poll(async () => (await prints(page)).some((h) => h.includes('Товарный чек'))).toBe(true)
      expect(plain((await prints(page)).find((h) => h.includes('Товарный чек')) ?? '')).not.toContain('Фильтр воздушный')
      if (mode.touch) await hit(page.getByRole('button', { name: 'Выйти из режима кассы' }))
      await page.goto(`/clients/${s.party.id}`)
      await expect(page.getByTestId('book-slots').getByRole('row', { name: /Фильтр воздушный/ })).toContainText(/на 231\s000 км/)
    })
  })
}
