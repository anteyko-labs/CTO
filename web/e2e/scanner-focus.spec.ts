// Клавиатурный сканер на кассе (ui-rules §4): код и Enter приходят туда, где фокус. После касания
// кнопки кассы (плитка, «+», «−», крестик, «Очистить», «Печать чека») или поля количества скан
// должен добавить отсканированный товар, а не нажать кнопку ещё раз. 26.10.2026 заказчик:
// «в чеке постоянно добавляет воздушный фильтр» — Enter сканера снова нажимал плитку или «+»
// у строки фильтра. Сканер здесь — как настоящий: знаки и Enter, без щелчка в поле поиска.
import { expect, test, type APIRequestContext, type Locator, type Page } from '@playwright/test'
import { ownerSafe } from './safe'
import { choosePerson, pickMaster } from './cashier'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e351' }
const RUN = Date.now().toString().slice(-8)
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

async function api<T>(request: APIRequestContext, method: 'GET' | 'POST' | 'PUT', path: string, data?: unknown): Promise<T> {
  const res = await request.fetch(`/api/v1${path}`, { method, data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

interface Goods {
  id: string
  name: string
  code: string
}

/** Товары проверки: воздушный и масляный фильтры штучно, два масла канистрами по 4 л. */
async function goods(request: APIRequestContext) {
  const tag = `${RUN}${++seq}`
  const filters = await api<{ id: string }>(request, 'POST', '/categories', { name: `Фильтры скан ${tag}`, kind: 'filter' })
  const oils = await api<{ id: string }>(request, 'POST', '/categories', { name: `Масла скан ${tag}`, kind: 'oil' })
  const safe = await ownerSafe(request)
  const make = async (name: string, code: string, category: string, extra: { container_ml?: number; price: number; qty: number; cost: number; filter?: string }): Promise<Goods> => {
    const p = await api<{ id: string }>(request, 'POST', '/products', {
      op_id: crypto.randomUUID(),
      category_id: category,
      name,
      barcodes: [code],
      container_ml: extra.container_ml,
      attrs: extra.filter ? { filter_type: extra.filter } : undefined,
      sale_price_tyiyn: extra.price,
    })
    await api(request, 'POST', '/receipts', { op_id: crypto.randomUUID(), paid_from: safe, lines: [{ product_id: p.id, qty: extra.qty, cost_tyiyn: extra.cost }] })
    return { id: p.id, name, code }
  }
  return {
    air: await make(`Фильтр воздушный скан ${tag}`, `46${tag}1`, filters.id, { price: 60_000, qty: 20, cost: 600_000, filter: 'воздушный' }),
    oilFilter: await make(`Фильтр масляный скан ${tag}`, `46${tag}2`, filters.id, { price: 40_000, qty: 20, cost: 400_000, filter: 'масляный' }),
    oil: await make(`Масло моторное скан ${tag}`, `46${tag}3`, oils.id, { container_ml: 4000, price: 300_000, qty: 40_000, cost: 2_000_000 }),
    oil2: await make(`Масло с подарком скан ${tag}`, `46${tag}4`, oils.id, { container_ml: 4000, price: 320_000, qty: 40_000, cost: 2_000_000 }),
  }
}

/** Плитки «ходовое» на время проверки; прежние возвращаются в конце. */
async function withTiles(request: APIRequestContext, ids: string[], run: () => Promise<void>) {
  const before = await api<{ kind: 'product' | 'service'; product?: { id: string }; service?: { id: string } }[]>(request, 'GET', '/settings/favorites')
  await api(request, 'PUT', '/settings/favorites', ids.map((id) => ({ kind: 'product', id })))
  try {
    await run()
  } finally {
    await api(
      request,
      'PUT',
      '/settings/favorites',
      before.flatMap((t) => {
        const id = t.kind === 'product' ? t.product?.id : t.service?.id
        return id ? [{ kind: t.kind, id }] : []
      }),
    )
  }
}

/** Скан: знаки кода и Enter приходят туда, где сейчас фокус, — в поле поиска не щёлкаем. */
async function scan(page: Page, code: string) {
  await page.keyboard.type(code, { delay: 5 })
  await page.keyboard.press('Enter')
}

const cart = (page: Page) => page.getByTestId('cart')
const row = (page: Page, name: string) => cart(page).getByRole('listitem').filter({ hasText: name })
const qty = (page: Page, name: string) => row(page, name).getByRole('textbox', { name: /^(Кол-во|Канистры)$/ })
const picker = (page: Page) => page.locator('[data-picker]').first()
const prints = (page: Page) => page.evaluate(() => (window as unknown as { __prints?: string[] }).__prints?.length ?? 0)

for (const mode of [
  { title: 'обычная касса', touch: false },
  { title: 'режим кассы 1366×768', touch: true },
] as const) {
  test.describe(`сканер после касания кнопки — ${mode.title}`, () => {
    test.use({ viewport: { width: 1366, height: 768 }, hasTouch: mode.touch })
    const hit = (l: Locator) => (mode.touch ? l.tap() : l.click())

    /** Кассир чека: при открытой смене — кассир смены (в режиме кассы он в полосе сверху). */
    async function pickCashier(page: Page, name: string) {
      const select = page.locator('#cashier-select')
      const onShift = page.getByTestId(mode.touch ? 'mode-shift' : 'shift-cashier')
      await expect(select.or(onShift)).toBeVisible()
      if (await select.isVisible()) await choosePerson(page, 'cashier-select', name)
    }

    async function open(page: Page) {
      await page.addInitScript(PRINT_STUB)
      await page.addInitScript((touch) => {
        if (touch) localStorage.setItem('avtodom.cashier_mode', '1')
        else localStorage.removeItem('avtodom.cashier_mode')
      }, mode.touch)
      await page.goto('/login')
      await page.getByLabel('Логин').fill(OWNER)
      await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
      await page.getByRole('button', { name: 'Войти' }).click()
      await expect(picker(page)).toBeVisible()
      if (mode.touch) await expect(page.getByTestId('cashier-mode-bar')).toBeVisible()
      await expect(picker(page)).toBeFocused()
    }

    test('плитка товара, затем скан: встаёт отсканированное, у плитки — одна штука', async ({ page }) => {
      await open(page)
      const g = await goods(page.request)
      await withTiles(page.request, [g.air.id, g.oil.id], async () => {
        await page.reload()
        await hit(page.getByRole('button', { name: new RegExp(`^${g.air.name}`) }))
        await expect(qty(page, g.air.name)).toHaveValue('1')
        await scan(page, g.oil.code)
        await expect(row(page, g.oil.name)).toBeVisible()
        await expect(qty(page, g.air.name)).toHaveValue('1')
        // Второй скан — ещё одна канистра масла, фильтр не прибавляется.
        await scan(page, g.oil.code)
        await expect(qty(page, g.oil.name)).toHaveValue('2')
        await expect(qty(page, g.air.name)).toHaveValue('1')
        await expect(picker(page)).toBeFocused()
      })
    })

    test('«Больше» у строки, затем скан: строка не прибавляется, отсканированное встаёт', async ({ page }) => {
      await open(page)
      const g = await goods(page.request)
      await scan(page, g.air.code)
      await expect(qty(page, g.air.name)).toHaveValue('1')
      await hit(row(page, g.air.name).getByRole('button', { name: 'Больше' }))
      await expect(qty(page, g.air.name)).toHaveValue('2')
      await scan(page, g.oil.code)
      await expect(row(page, g.oil.name)).toBeVisible()
      await expect(qty(page, g.air.name)).toHaveValue('2')
      await expect(picker(page)).toBeFocused()
    })

    test('«Меньше» у строки, затем скан: количество не меняется, отсканированное встаёт', async ({ page }) => {
      await open(page)
      const g = await goods(page.request)
      await scan(page, g.air.code)
      const plus = row(page, g.air.name).getByRole('button', { name: 'Больше' })
      await hit(plus)
      await hit(plus)
      await expect(qty(page, g.air.name)).toHaveValue('3')
      await hit(row(page, g.air.name).getByRole('button', { name: 'Меньше' }))
      await expect(qty(page, g.air.name)).toHaveValue('2')
      await scan(page, g.oil.code)
      await expect(row(page, g.oil.name)).toBeVisible()
      await expect(qty(page, g.air.name)).toHaveValue('2')
    })

    test('крестик у строки, затем скан: скан не теряется', async ({ page }) => {
      await open(page)
      const g = await goods(page.request)
      await scan(page, g.air.code)
      await scan(page, g.oil.code)
      await expect(row(page, g.oil.name)).toBeVisible()
      await hit(row(page, g.oil.name).getByRole('button', { name: 'Удалить строку' }))
      await expect(row(page, g.oil.name)).toHaveCount(0)
      await scan(page, g.oilFilter.code)
      await expect(row(page, g.oilFilter.name)).toBeVisible()
      await expect(qty(page, g.air.name)).toHaveValue('1')
    })

    test('поле количества в фокусе, затем скан: код не уходит в количество', async ({ page }) => {
      await open(page)
      const g = await goods(page.request)
      await scan(page, g.air.code)
      await hit(qty(page, g.air.name))
      await expect(qty(page, g.air.name)).toBeFocused()
      await scan(page, g.oil.code)
      await expect(row(page, g.oil.name)).toBeVisible()
      await expect(qty(page, g.air.name)).toHaveValue('1')
      await expect(picker(page)).toBeFocused()
    })

    test('«Получено» в фокусе, затем скан: чек не проводится, код не уходит в сумму', async ({ page }) => {
      await open(page)
      const g = await goods(page.request)
      const cashier = `Кассир скан ${RUN}${++seq}`
      await api(page.request, 'POST', '/employees', { full_name: cashier, is_cashier: true, pin: '1234' })
      await page.reload()
      await pickCashier(page, cashier)
      await picker(page).focus()
      await scan(page, g.air.code)
      await expect(qty(page, g.air.name)).toHaveValue('1')
      const received = page.locator('[data-received]')
      await hit(received)
      await expect(received).toBeFocused()
      await scan(page, g.oilFilter.code)
      await expect(row(page, g.oilFilter.name)).toBeVisible()
      await expect(received).toHaveValue('')
      await expect(page.getByText(/Чек № .+ проведён/)).toHaveCount(0)
    })

    test('«Очистить», затем скан: чек не стирается', async ({ page }) => {
      await open(page)
      const g = await goods(page.request)
      await scan(page, g.air.code)
      await hit(page.getByRole('button', { name: 'Очистить', exact: true }))
      await expect(page.getByRole('button', { name: 'Точно очистить?' })).toBeVisible()
      await scan(page, g.oil.code)
      await expect(row(page, g.oil.name)).toBeVisible()
      await expect(qty(page, g.air.name)).toHaveValue('1')
    })

    test('плитка открыла окно подарка, затем скан: товар встаёт за окном, плитка не нажимается снова', async ({ page }) => {
      await open(page)
      const g = await goods(page.request)
      await api(page.request, 'POST', '/gift-rules', { trigger_product_id: g.oil2.id, min_units: 4000, items: [{ gift_product_id: g.oilFilter.id, gift_qty: 1 }] })
      await withTiles(page.request, [g.oil2.id], async () => {
        await page.reload()
        await hit(page.getByRole('button', { name: new RegExp(`^${g.oil2.name}`) }))
        const dialog = page.getByRole('dialog', { name: 'Выберите подарок' })
        await expect(dialog).toBeVisible()
        await scan(page, g.air.code)
        await expect(row(page, g.air.name)).toBeVisible()
        await expect(qty(page, g.oil2.name)).toHaveValue('1')
        await expect(dialog).toBeVisible()
        await hit(dialog.getByRole('button', { name: new RegExp(g.oilFilter.name) }))
        await expect(row(page, g.oilFilter.name)).toContainText('подарок')
        await expect(qty(page, g.oil2.name)).toHaveValue('1')
      })
    })

    test('окно подарка встало, пока фокус в количестве, затем скан: количество не меняется, товар встаёт', async ({ page }) => {
      await open(page)
      const g = await goods(page.request)
      await api(page.request, 'POST', '/gift-rules', { trigger_product_id: g.oil2.id, min_units: 8000, items: [{ gift_product_id: g.oilFilter.id, gift_qty: 1 }] })
      await scan(page, g.oil2.code)
      await expect(qty(page, g.oil2.name)).toHaveValue('1')
      // Две канистры — порог набран, окно подарка встаёт само; фокус остаётся в количестве.
      await qty(page, g.oil2.name).fill('2')
      const dialog = page.getByRole('dialog', { name: 'Выберите подарок' })
      await expect(dialog).toBeVisible()
      await expect(qty(page, g.oil2.name)).toBeFocused()
      await scan(page, g.air.code)
      await expect(row(page, g.air.name)).toBeVisible()
      await expect(qty(page, g.oil2.name)).toHaveValue('2')
      await expect(dialog).toBeVisible()
    })

    test('окно закрыли («Без подарка», «Готово» в «Вид замены»), затем скан: скан не теряется', async ({ page }) => {
      await open(page)
      const g = await goods(page.request)
      await api(page.request, 'POST', '/gift-rules', { trigger_product_id: g.oil2.id, min_units: 4000, items: [{ gift_product_id: g.oilFilter.id, gift_qty: 1 }] })
      const service = `Замена масла в моторе скан ${RUN}${++seq}`
      await api(page.request, 'POST', '/services', { name: service, price_tyiyn: 20_000, master_fee_tyiyn: 3_000 })
      await scan(page, g.oil2.code)
      const gift = page.getByRole('dialog', { name: 'Выберите подарок' })
      await expect(gift).toBeVisible()
      await hit(gift.getByRole('button', { name: 'Без подарка' }))
      await expect(gift).toHaveCount(0)
      await scan(page, g.air.code)
      await expect(row(page, g.air.name)).toBeVisible()

      await hit(page.getByRole('button', { name: 'В сервис' }))
      const kinds = page.getByRole('dialog', { name: 'Вид замены' })
      await kinds.getByPlaceholder('Найти услугу').fill(service)
      await hit(kinds.getByTestId('service-choices').getByRole('button', { name: new RegExp(service) }))
      await hit(kinds.getByRole('button', { name: 'Готово' }))
      await expect(kinds).toHaveCount(0)
      await scan(page, g.oil.code)
      await expect(row(page, g.oil.name)).toBeVisible()
      await expect(qty(page, g.air.name)).toHaveValue('1')
    })

    test('комментарий в фокусе, затем скан: код не уходит в комментарий, набор руками — уходит', async ({ page }) => {
      await open(page)
      const g = await goods(page.request)
      await scan(page, g.air.code)
      await hit(page.getByRole('button', { name: mode.touch ? 'Комментарий' : '+ комментарий', exact: true }))
      const comment = page.getByRole('textbox', { name: 'Комментарий' })
      await expect(comment).toBeFocused()
      await scan(page, g.oil.code)
      await expect(row(page, g.oil.name)).toBeVisible()
      await expect(comment).toHaveValue('')
      await comment.focus()
      await page.keyboard.type('Клиент ждёт', { delay: 90 })
      await expect(comment).toHaveValue('Клиент ждёт')
    })

    test('способ оплаты, затем скан: способ не меняется, отсканированное встаёт', async ({ page }) => {
      await open(page)
      const g = await goods(page.request)
      await scan(page, g.air.code)
      const card = page.locator('[data-pay]').getByRole('button', { name: 'Карта / QR банка', exact: true })
      await hit(card)
      await expect(card).toHaveAttribute('aria-pressed', 'true')
      await scan(page, g.oil.code)
      await expect(row(page, g.oil.name)).toBeVisible()
      await expect(card).toHaveAttribute('aria-pressed', 'true')
      await expect(qty(page, g.air.name)).toHaveValue('1')
    })

    test('«+ ещё строку» в «Следующей замене», её окно и «✕», затем скан: товар встаёт, окно и строки не меняются', async ({ page }) => {
      test.setTimeout(90_000)
      await open(page)
      const g = await goods(page.request)
      const tag = `${RUN}${++seq}`
      // Замена масла в коробке: с маслом двигателя фильтры своих строк не дают (ADR-117), а здесь нужны.
      const service = `Замена масла в коробке ещё скан ${tag}`
      await api(page.request, 'POST', '/services', { name: service, price_tyiyn: 20_000, master_fee_tyiyn: 3_000 })
      const last = `Скан${tag}`
      const party = await api<{ id: string }>(page.request, 'POST', '/parties', { kind: 'person', first_name: 'Азиз', last_name: last, phone: `0703${tag.slice(-6)}` })
      const plate = `09KG${tag.slice(-3)}SCN`
      const vehicle = await api<{ id: string }>(page.request, 'POST', `/parties/${party.id}/vehicles`, { plate, brand: 'Toyota', model: 'Camry' })
      await page.reload()
      await expect(picker(page)).toBeFocused()
      await scan(page, g.air.code)
      await expect(row(page, g.air.name)).toBeVisible()
      await hit(page.getByRole('button', { name: 'В сервис' }))
      const kinds = page.getByRole('dialog', { name: 'Вид замены' })
      await kinds.getByPlaceholder('Найти услугу').fill(service)
      await hit(kinds.getByTestId('service-choices').getByRole('button', { name: new RegExp(service) }))
      await hit(kinds.getByRole('button', { name: 'Готово' }))
      await expect(kinds).toHaveCount(0)
      if (mode.touch) {
        await hit(page.getByTestId('service-left').getByRole('button', { name: 'Выбрать клиента' }))
        const dlg = page.getByRole('dialog', { name: 'Клиент и машина' })
        await dlg.locator('[data-client-input]').fill(plate)
        await hit(dlg.getByRole('button', { name: new RegExp(last) }).first())
        await expect(dlg.getByLabel('Машина')).toHaveValue(vehicle.id)
        await hit(dlg.getByRole('button', { name: 'Готово' }))
      } else {
        await page.locator('[data-client-input]').fill(plate)
        await page.getByRole('button', { name: new RegExp(last) }).first().click()
      }
      await page.locator('#mileage').fill('217000')
      const block = page.getByTestId('next-change')
      const fields = block.locator('input[id^="next-"]')
      await expect(fields).toHaveCount(1)
      const more = block.getByRole('button', { name: '+ ещё строку' })
      const extra = page.getByRole('dialog', { name: 'Следующая замена: ещё строка' })

      // Окно открыто — скан встаёт в чек за окном; варианты окна — из строк чека, строк не прибавилось.
      await hit(more)
      await expect(extra).toBeVisible()
      // Кнопка «⌨» у поля поиска за окном не ложится поверх окна.
      await expect(page.getByTestId('keyboard-field-button')).toHaveCount(0)
      await scan(page, g.oilFilter.code)
      await expect(row(page, g.oilFilter.name)).toBeVisible()
      await expect(extra).toBeVisible()
      await expect(extra.getByRole('button', { name: /Фильтр масляный/ })).toBeVisible()
      await expect(fields).toHaveCount(1)

      // Окно закрыли без выбора («Отмена» под пальцем, Escape мышью) — скан не теряется и окно не открывает.
      if (mode.touch) await hit(extra.getByRole('button', { name: 'Отмена' }))
      else await page.keyboard.press('Escape')
      await expect(extra).toHaveCount(0)
      await scan(page, g.oil.code)
      await expect(row(page, g.oil.name)).toBeVisible()
      await expect(extra).toHaveCount(0)
      await expect(fields).toHaveCount(1)

      // Выбрали позицию — строка встала; скан добавляет товар, окно не открывается, строк столько же.
      await hit(more)
      await hit(extra.getByRole('button', { name: /Фильтр воздушный/ }))
      await expect(extra).toHaveCount(0)
      await expect(fields).toHaveCount(2)
      await scan(page, g.oil2.code)
      await expect(row(page, g.oil2.name)).toBeVisible()
      await expect(extra).toHaveCount(0)
      await expect(fields).toHaveCount(2)

      // «✕» у строки — строка ушла; скан прибавляет отсканированное, «✕» и «+ ещё» не нажимаются.
      await hit(block.getByRole('button', { name: /^Убрать строку Фильтр воздушный/ }))
      await expect(fields).toHaveCount(1)
      await scan(page, g.air.code)
      await expect(qty(page, g.air.name)).toHaveValue('2')
      await expect(extra).toHaveCount(0)
      await expect(fields).toHaveCount(1)
    })

    if (mode.touch) {
      test('выбрали мастера кнопкой, затем скан: товар встаёт', async ({ page }) => {
        await open(page)
        const g = await goods(page.request)
        const service = `Замена масла в моторе мастер ${RUN}${++seq}`
        await api(page.request, 'POST', '/services', { name: service, price_tyiyn: 20_000, master_fee_tyiyn: 3_000 })
        const master = `Мастер скан ${RUN}${++seq}`
        await api(page.request, 'POST', '/employees', { full_name: master, is_master: true })
        await page.reload()
        await page.getByRole('button', { name: 'В сервис' }).tap()
        const kinds = page.getByRole('dialog', { name: 'Вид замены' })
        await kinds.getByPlaceholder('Найти услугу').fill(service)
        await kinds.getByTestId('service-choices').getByRole('button', { name: new RegExp(service) }).tap()
        await kinds.getByRole('button', { name: 'Готово' }).tap()
        // Мастер — кнопками режима кассы (SPEC-36 §16): после выбора скан идёт в чек.
        await pickMaster(page, master)
        await scan(page, g.oil.code)
        await expect(row(page, g.oil.name)).toBeVisible()
        await expect(page.locator('#master-select')).toHaveAttribute('data-value', /.+/)
      })

      test('экранная клавиатура открыта: плитка, затем скан', async ({ page }) => {
        await open(page)
        const g = await goods(page.request)
        await withTiles(page.request, [g.air.id], async () => {
          await page.reload()
          await page.getByTestId('cashier-mode-bar').getByRole('button', { name: '⌨ Клавиатура' }).tap()
          await expect(page.getByTestId('keyboard')).toBeVisible()
          await page.getByRole('button', { name: new RegExp(`^${g.air.name}`) }).tap()
          await expect(qty(page, g.air.name)).toHaveValue('1')
          await scan(page, g.oil.code)
          await expect(row(page, g.oil.name)).toBeVisible()
          await expect(qty(page, g.air.name)).toHaveValue('1')
        })
      })

      test('«Печать чека» после проведения, затем скан: чек не печатается второй раз, товар в новом чеке', async ({ page }) => {
        await open(page)
        const g = await goods(page.request)
        const cashier = `Кассир печать скан ${RUN}${++seq}`
        await api(page.request, 'POST', '/employees', { full_name: cashier, is_cashier: true, pin: '1234' })
        await page.reload()
        await pickCashier(page, cashier)
        await picker(page).focus()
        await scan(page, g.air.code)
        await expect(qty(page, g.air.name)).toHaveValue('1')
        await page.getByRole('button', { name: 'Без сдачи' }).tap()
        await page.getByRole('button', { name: /Провести чек/ }).tap()
        await expect(page.getByText(/Чек № .+ проведён/)).toBeVisible()
        await page.getByRole('button', { name: 'Печать чека' }).tap()
        await expect.poll(() => prints(page)).toBe(1)
        await scan(page, g.oil.code)
        await expect(row(page, g.oil.name)).toBeVisible()
        expect(await prints(page)).toBe(1)
      })
    } else {
      test('набор руками в поле количества и «Получено» не теряется', async ({ page }) => {
        await open(page)
        const g = await goods(page.request)
        await scan(page, g.air.code)
        const q = qty(page, g.air.name)
        await q.fill('')
        await page.keyboard.type('3', { delay: 120 })
        await expect(q).toHaveValue('3')
        // Быстро две цифры подряд — не скан: обе остаются в поле.
        await page.keyboard.type('12', { delay: 5 })
        await expect(q).toHaveValue('312')
        const received = page.locator('[data-received]')
        await received.click()
        await page.keyboard.type('5000', { delay: 90 })
        await expect(received).toHaveValue('5000')
        // Длинное число, набранное быстро без Enter, тоже остаётся в поле.
        await received.fill('')
        await page.keyboard.type('1234567', { delay: 5 })
        await expect(received).toHaveValue('1234567')
        await expect(row(page, g.air.name)).toHaveCount(1)
      })
    }
  })
}
