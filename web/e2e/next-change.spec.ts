// SPEC-34: «Следующая замена» по каждой услуге чека «В сервис» — заполнена заранее (пробег +
// интервал), кассир правит или очищает, меньше пробега — нельзя; цель в книжке машины, в печатном
// чеке и при повторной печати; без сети — уходит с чеком и печатается из очереди. Режим кассы
// 1366×768 — блок помещается без прокрутки страницы, цели касания ≥ 56.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { pickCashier } from './cashier'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e341' }
const RUN = Date.now().toString().slice(-8)
const SHOTS = process.env.E2E_SHOTS_DIR

/** Печать идёт через скрытый фрейм: HTML каждого печатаемого документа — в window.__prints. */
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

async function login(page: Page) {
  await page.addInitScript(PRINT_STUB)
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' }).first()).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'GET' | 'POST', path: string, data?: unknown): Promise<T> {
  const res = await request.fetch(`/api/v1${path}`, { method, data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

interface Slot {
  slot: string
  next_km: number | null
  next_mileage?: number | null
  next_date: string | null
}

async function setup(request: APIRequestContext, tag: string) {
  const oil = `Замена масла в моторе ${tag} ${RUN}`
  const anti = `Замена антифриза ${tag} ${RUN}`
  const svcOil = await api<{ id: string; book_slot: string; interval_km: number }>(request, 'POST', '/services', { name: oil, price_tyiyn: 20_000, master_fee_tyiyn: 3_000 })
  const svcAnti = await api<{ id: string; book_slot: string; interval_km: number }>(request, 'POST', '/services', { name: anti, price_tyiyn: 50_000, master_fee_tyiyn: 3_000 })
  // Строка книжки — по названию услуги (SPEC-34 §1).
  expect([svcOil.book_slot, svcOil.interval_km, svcAnti.book_slot, svcAnti.interval_km]).toEqual(['oil', 8000, 'antifreeze', 60000])
  const master = `Мастер ${tag} ${RUN}`
  await api(request, 'POST', '/employees', { full_name: master, is_master: true })
  const cashier = `Кассир ${tag} ${RUN}`
  await api(request, 'POST', '/employees', { full_name: cashier, is_cashier: true, pin: '1234' })
  const last = `След${tag}${RUN}`
  const party = await api<{ id: string }>(request, 'POST', '/parties', { kind: 'person', first_name: 'Азиз', last_name: last, phone: '0700123456' })
  const plate = `01KG${RUN.slice(-3)}${tag.toUpperCase().slice(0, 3)}`
  const vehicle = await api<{ id: string }>(request, 'POST', `/parties/${party.id}/vehicles`, { plate, brand: 'Toyota', model: 'Camry' })
  return { oil, anti, master, cashier, last, plate, party, vehicle }
}

async function addKinds(page: Page, names: string[], tap: boolean) {
  const kinds = page.getByRole('dialog', { name: 'Вид замены' })
  for (const n of names) {
    await kinds.getByPlaceholder('Найти услугу').fill(n)
    const b = kinds.getByTestId('service-choices').getByRole('button', { name: new RegExp(n) })
    if (tap) await b.tap()
    else await b.click()
  }
  const done = kinds.getByRole('button', { name: 'Готово' })
  if (tap) await done.tap()
  else await done.click()
}

test.describe('режим кассы 1366×768', () => {
  test.use({ viewport: { width: 1366, height: 768 }, hasTouch: true })

  test('«Следующая замена» слева под пробегом: без прокрутки страницы, поля под палец, меньше пробега — нельзя', async ({ page }) => {
    await login(page)
    const s = await setup(page.request, 'k')
    await page.goto('/')
    await page.evaluate(() => localStorage.setItem('avtodom.cashier_mode', '1'))
    await page.reload()
    await expect(page.getByTestId('cashier-mode-bar')).toBeVisible()
    await page.getByRole('button', { name: 'В сервис' }).tap()
    await addKinds(page, [s.oil, s.anti], true)
    const left = page.getByTestId('service-left')
    await left.getByRole('button', { name: 'Выбрать клиента' }).tap()
    const dlg = page.getByRole('dialog', { name: 'Клиент и машина' })
    await dlg.locator('[data-client-input]').fill(s.last)
    await dlg.getByRole('button', { name: new RegExp(s.last) }).first().tap()
    await dlg.getByRole('button', { name: 'Готово' }).tap()
    await left.locator('#master-select').selectOption({ label: s.master })
    await left.locator('#mileage').fill('217000')

    const block = left.getByTestId('next-change')
    await expect(block).toBeVisible()
    const oilField = block.getByLabel(`Следующая замена: ${s.oil}`)
    const antiField = block.getByLabel(`Следующая замена: ${s.anti}`)
    await expect(oilField).toHaveValue('225000')
    await expect(antiField).toHaveValue('277000')
    // Цифры — экранной клавиатурой по кнопке (ADR-101): своя клавиатура планшета не открывается.
    await expect(oilField).toHaveAttribute('inputmode', 'none')
    for (const l of [oilField, antiField, block.getByRole('button', { name: '+ ещё строку' }), left.locator('#mileage')]) {
      expect((await l.boundingBox())!.height).toBeGreaterThanOrEqual(56)
    }
    await expect(block).toBeInViewport({ ratio: 1 })
    const noScroll = await page.evaluate(() => {
      const el = document.scrollingElement ?? document.documentElement
      return el.scrollHeight <= window.innerHeight + 1 && el.scrollWidth <= window.innerWidth + 1
    })
    expect(noScroll).toBe(true)
    await expect(page.getByRole('button', { name: /Провести чек/ })).toBeInViewport()
    if (SHOTS) await page.screenshot({ path: `${SHOTS}/next-change-1366.png` })

    // Меньше пробега сейчас — «Провести» нельзя, подсказка у поля.
    await oilField.fill('200000')
    await expect(block.getByText(/не меньше пробега сейчас/)).toBeVisible()
    await expect(page.getByRole('button', { name: /Провести чек/ })).toBeDisabled()
    if (SHOTS) await page.screenshot({ path: `${SHOTS}/next-change-1366-low.png` })
    await oilField.fill('225000')
    await expect(block.getByText(/не меньше пробега сейчас/)).toHaveCount(0)
    await expect(page.getByRole('button', { name: /Провести чек/ })).toBeEnabled()

    // «+ ещё строку» — услуга, которой нет в чеке; убирается крестиком.
    await block.getByRole('button', { name: '+ ещё строку' }).tap()
    const extra = page.getByTestId('next-extra-choices')
    await expect(extra).toBeVisible()
    await expect(extra.getByRole('button', { name: new RegExp(s.oil) })).toHaveCount(0)
    await page.keyboard.press('Escape')

    await page.getByRole('button', { name: 'Очистить', exact: true }).tap()
    await page.getByRole('button', { name: 'Точно очистить?' }).tap()
    await page.getByRole('button', { name: 'Выйти из режима кассы' }).tap()
  })
})

test('чек «В сервис»: следующая замена в книжке, в чеке и при повторной печати; без сети — из очереди', async ({ page }) => {
  await login(page)
  const s = await setup(page.request, 'd')
  const sell = async (antiValue: string) => {
    await page.goto('/')
    await page.getByRole('button', { name: 'В сервис' }).click()
    await addKinds(page, [s.oil, s.anti], false)
    await pickCashier(page, s.cashier)
    await page.locator('#master-select').selectOption({ label: s.master })
    await page.locator('[data-client-input]').fill(s.plate)
    await page.getByRole('button', { name: new RegExp(s.last) }).first().click()
    await page.locator('#mileage').fill('217000')
    const block = page.getByTestId('next-change')
    await expect(block.getByLabel(`Следующая замена: ${s.oil}`)).toHaveValue('225000')
    await block.getByLabel(`Следующая замена: ${s.anti}`).fill(antiValue)
    await page.getByRole('button', { name: 'Без сдачи' }).click()
    await page.getByRole('button', { name: 'Провести чек' }).click()
    await expect(page.getByText(/Чек № .+ проведён/)).toBeVisible()
  }

  await sell('250000')
  const book = await api<{ slots: Slot[] }>(page.request, 'GET', `/vehicles/${s.vehicle.id}/oil-book`)
  const slot = (n: string) => book.slots.find((x) => x.slot === n)
  expect(slot('oil')?.next_km).toBe(225000)
  expect(slot('antifreeze')?.next_km).toBe(250000)

  await page.getByRole('button', { name: 'Печать чека' }).click()
  await expect.poll(async () => (await prints(page)).some((h) => h.includes('Масляная книжка') && h.includes('Товарный чек'))).toBe(true)
  const receipt = (await prints(page)).filter((h) => h.includes('Товарный чек')).at(-1) ?? ''
  const text = plain(receipt)
  expect(text).toContain(`Машина: Toyota Camry, ${s.plate}`)
  expect(text).toContain('Пробег сейчас: 217 000 км')
  expect(text).toMatch(new RegExp(`${s.oil} — следующая замена на пробеге 225 000 км или до \\d\\d\\.\\d\\d\\.\\d{4}`))
  expect(text).toContain(`${s.anti} — следующая замена на пробеге 250 000 км`)
  if (SHOTS) {
    const view = await page.context().newPage()
    await view.setViewportSize({ width: 320, height: 700 })
    await view.setContent(receipt)
    await view.screenshot({ path: `${SHOTS}/next-change-receipt.png`, fullPage: true })
    await view.close()
  }

  // Повторная печать из «Чеков».
  const href = await page.getByRole('link', { name: 'Открыть', exact: true }).getAttribute('href')
  await page.goto(href ?? '/')
  await page.getByRole('button', { name: 'Печать', exact: true }).click()
  await expect.poll(async () => (await prints(page)).filter((h) => h.includes('Масляная книжка')).length).toBeGreaterThan(0)
  expect(plain((await prints(page)).at(-1) ?? '')).toContain(`${s.anti} — следующая замена на пробеге 250 000 км`)

  // Без сети: чек в очереди, книжка — в чеке из очереди; пустое поле не печатается.
  await page.route('**/api/v1/sales', (r) => (r.request().method() === 'POST' ? r.abort() : r.continue()))
  await sell('')
  await page.getByRole('button', { name: 'Печать чека' }).click()
  await expect.poll(async () => (await prints(page)).filter((h) => h.includes('Масляная книжка')).length).toBeGreaterThan(0)
  const queued = plain((await prints(page)).at(-1) ?? '')
  expect(queued).toContain(`${s.oil} — следующая замена на пробеге 225 000 км`)
  expect(queued).not.toContain(`${s.anti} — следующая замена`)
  await page.unroute('**/api/v1/sales')
  // Очередь отправит сама: антифриз — без напоминания, масло — 225 000.
  await expect
    .poll(
      async () => {
        const b = await api<{ records: { sale_id: string | null }[]; slots: Slot[] }>(page.request, 'GET', `/vehicles/${s.vehicle.id}/oil-book`)
        return [b.records.length, b.slots.find((x) => x.slot === 'antifreeze')?.next_km ?? null, b.slots.find((x) => x.slot === 'oil')?.next_km]
      },
      { timeout: 60_000, intervals: [1000] },
    )
    .toEqual([2, null, 225000])
})
