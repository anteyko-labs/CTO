// ADR-131 (заказчик 27.10.2026): «в еще строку должно показываться только услуги которые набрались в
// этом чеке в сервис, а не все подряд». В чеке замена масла, «Замена ГУР», «Смазка кардана» (услуги
// «прочее»), масло, ATF и антифриз: строкой встаёт только масло; «+ ещё» предлагает только ГУР и
// кардан — ни товаров чека (ATF и антифриз без своих услуг — доливка), ни справочника; строка
// «прочее» — с названием услуги, у кардана заполнена по его интервалу; уходит с чеком в книжку
// (печатный чек — только жидкости); удалили услугу из чека — строка ушла.
import { expect, test, type APIRequestContext, type Locator, type Page } from '@playwright/test'
import { ownerSafe } from './safe'
import { choosePerson, pickMaster } from './cashier'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e356' }
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

async function api<T>(request: APIRequestContext, path: string, data?: unknown): Promise<T> {
  const res = data === undefined ? await request.get(`/api/v1${path}`, { headers: DEVICE }) : await request.post(`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

/** Масло, ATF, антифриз, топливный фильтр и тормозная жидкость на складе; в справочнике — замена масла,
 *  замена антифриза и три услуги «прочее»: ГУР (без интервала), кардан (20 000 км или 180 дней) и
 *  промывка инжектора. */
async function setup(request: APIRequestContext) {
  const tag = `${RUN}${++seq}`
  const safe = await ownerSafe(request)
  const oilCat = await api<{ id: string }>(request, '/categories', { name: `Масла ещё ${tag}`, kind: 'oil' })
  const filterCat = await api<{ id: string }>(request, '/categories', { name: `Фильтры ещё ${tag}`, kind: 'filter' })
  const antiCat = await api<{ id: string }>(request, '/categories', { name: `Антифризы ещё ${tag}`, kind: 'other', book: { slot: 'antifreeze', interval_km: 60_000, interval_days: 730 } })
  // Строки книжки — по названию категории (ADR-117).
  const gearCat = await api<{ id: string; book_slot: string }>(request, '/categories', { name: `Трансмиссионные масла ещё ${tag}`, kind: 'oil' })
  const brakeCat = await api<{ id: string; book_slot: string }>(request, '/categories', { name: `Тормозные жидкости ещё ${tag}`, kind: 'other' })
  expect([gearCat.book_slot, brakeCat.book_slot]).toEqual(['gear_oil', 'brake_fluid'])
  const oilCode = `46${tag}1`
  const fuelCode = `46${tag}2`
  const antiCode = `46${tag}3`
  const gearCode = `46${tag}4`
  const brakeCode = `46${tag}5`
  const oil = await api<{ id: string }>(request, '/products', { op_id: crypto.randomUUID(), category_id: oilCat.id, name: `Масло ещё ${tag} 5W-30`, container_ml: 4000, barcodes: [oilCode], sale_price_tyiyn: 300_000 })
  const fuel = await api<{ id: string }>(request, '/products', { op_id: crypto.randomUUID(), category_id: filterCat.id, name: `Фильтр топливный ещё ${tag}`, attrs: { filter_type: 'топливный' }, barcodes: [fuelCode], sale_price_tyiyn: 60_000 })
  const anti = await api<{ id: string }>(request, '/products', { op_id: crypto.randomUUID(), category_id: antiCat.id, name: `Антифриз ещё ${tag}`, barcodes: [antiCode], sale_price_tyiyn: 90_000 })
  const gear = await api<{ id: string }>(request, '/products', { op_id: crypto.randomUUID(), category_id: gearCat.id, name: `ATF ещё ${tag} Dexron`, container_ml: 1000, barcodes: [gearCode], sale_price_tyiyn: 40_000 })
  const brake = await api<{ id: string }>(request, '/products', { op_id: crypto.randomUUID(), category_id: brakeCat.id, name: `Тормозная жидкость ещё ${tag}`, barcodes: [brakeCode], sale_price_tyiyn: 50_000 })
  await api(request, '/receipts', {
    op_id: crypto.randomUUID(),
    paid_from: safe,
    lines: [
      { product_id: oil.id, qty: 40_000, cost_tyiyn: 2_000_000 },
      { product_id: fuel.id, qty: 10, cost_tyiyn: 300_000 },
      { product_id: anti.id, qty: 5, cost_tyiyn: 300_000 },
      { product_id: gear.id, qty: 10_000, cost_tyiyn: 200_000 },
      { product_id: brake.id, qty: 10, cost_tyiyn: 250_000 },
    ],
  })
  const service = `Замена масла в моторе ещё ${tag}`
  const svcOil = await api<{ id: string }>(request, '/services', { name: service, price_tyiyn: 20_000, master_fee_tyiyn: 3_000 })
  const antiService = `Замена антифриза ещё ${tag}`
  const svcAnti = await api<{ id: string; book_slot: string }>(request, '/services', { name: antiService, price_tyiyn: 50_000, master_fee_tyiyn: 3_000 })
  expect(svcAnti.book_slot).toBe('antifreeze')
  // Услуги «прочее» (строку задаёт владелец; по названию они тоже «прочее»).
  const other = async (name: string, interval?: { km: number; days: number }) =>
    api<{ id: string; book_slot: string }>(request, '/services', {
      name,
      price_tyiyn: 30_000,
      master_fee_tyiyn: 3_000,
      book_slot: 'other',
      ...(interval ? { book_interval_km: interval.km, book_interval_days: interval.days } : {}),
    })
  const gur = `Замена ГУР ещё ${tag}`
  const shaft = `Смазка кардана ещё ${tag}`
  const flush = `Промывка инжектора ещё ${tag}`
  const svcGur = await other(gur)
  await other(shaft, { km: 20_000, days: 180 })
  await other(flush)
  expect(svcGur.book_slot).toBe('other')
  const master = `Мастер ещё ${tag}`
  await api(request, '/employees', { full_name: master, is_master: true })
  const cashier = `Кассир ещё ${tag}`
  await api(request, '/employees', { full_name: cashier, is_cashier: true, pin: '1234' })
  const last = `Ещё${tag}`
  const party = await api<{ id: string }>(request, '/parties', { kind: 'person', first_name: 'Азиз', last_name: last, phone: `0702${tag.slice(-6)}` })
  const plate = `08KG${tag.slice(-3)}AFQ`
  const vehicle = await api<{ id: string }>(request, `/parties/${party.id}/vehicles`, { plate, brand: 'Toyota', model: 'Camry' })
  return { oilCode, fuelCode, antiCode, gearCode, brakeCode, service, serviceId: svcOil.id, antiService, antiServiceId: svcAnti.id, gur, gurId: svcGur.id, shaft, flush, master, cashier, last, plate, vehicle }
}

type Setup = Awaited<ReturnType<typeof setup>>

/** Плитки «ходовое» на время проверки; прежние возвращаются в конце. */
async function withTiles(request: APIRequestContext, tiles: { kind: 'product' | 'service'; id: string }[], run: () => Promise<void>) {
  const before = await api<{ kind: 'product' | 'service'; product?: { id: string }; service?: { id: string } }[]>(request, '/settings/favorites')
  const put = async (data: unknown) => {
    const res = await request.put('/api/v1/settings/favorites', { data, headers: DEVICE })
    expect(res.ok(), await res.text()).toBeTruthy()
  }
  await put(tiles)
  try {
    await run()
  } finally {
    await put(
      before.flatMap((t) => {
        const id = t.kind === 'product' ? t.product?.id : t.service?.id
        return id ? [{ kind: t.kind, id }] : []
      }),
    )
  }
}

/** Вход владельцем в обычной кассе или в режиме кассы. */
async function login(page: Page, touch: boolean) {
  await page.addInitScript(PRINT_STUB)
  await page.addInitScript((t) => {
    if (t) localStorage.setItem('avtodom.cashier_mode', '1')
    else localStorage.removeItem('avtodom.cashier_mode')
  }, touch)
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.locator('[data-picker]')).toBeVisible()
}

/** Виды замены в открытом окне «Вид замены» и «Готово». */
async function addKinds(page: Page, names: string[], hit: (l: Locator) => Promise<void>) {
  const kinds = page.getByRole('dialog', { name: 'Вид замены' })
  for (const n of names) {
    await kinds.getByPlaceholder('Найти услугу').fill(n)
    await hit(kinds.getByTestId('service-choices').getByRole('button', { name: new RegExp(n) }))
  }
  await hit(kinds.getByRole('button', { name: 'Готово' }))
  await expect(kinds).toHaveCount(0)
}

/** Кассир, мастер, клиент с машиной и пробег 217 000 в чеке «В сервис». */
async function serviceDetails(page: Page, s: Setup, touch: boolean, hit: (l: Locator) => Promise<void>) {
  const select = page.locator('#cashier-select')
  if (await select.isVisible()) await choosePerson(page, 'cashier-select', s.cashier)
  await pickMaster(page, s.master)
  if (touch) {
    await hit(page.getByTestId('service-left').getByRole('button', { name: 'Выбрать клиента' }))
    const dlg = page.getByRole('dialog', { name: 'Клиент и машина' })
    await dlg.locator('[data-client-input]').fill(s.plate)
    await hit(dlg.getByRole('button', { name: new RegExp(s.last) }).first())
    // Машина встаёт сама, когда загрузится карточка клиента: «Готово» раньше — окно закроется без неё.
    await expect(dlg.getByLabel('Машина')).toHaveValue(s.vehicle.id)
    await hit(dlg.getByRole('button', { name: 'Готово' }))
  } else {
    await page.locator('[data-client-input]').fill(s.plate)
    await page.getByRole('button', { name: new RegExp(s.last) }).first().click()
  }
  await page.locator('#mileage').fill('217000')
}

/** Страница не прокручивается ни вниз, ни вбок. */
const noScroll = (page: Page) =>
  page.evaluate(() => {
    const el = document.scrollingElement ?? document.documentElement
    return el.scrollHeight <= window.innerHeight + 1 && el.scrollWidth <= window.innerWidth + 1
  })

for (const mode of [
  { title: 'обычная касса', touch: false },
  { title: 'режим кассы 1366×768', touch: true },
] as const) {
  test.describe(mode.title, () => {
    test.use({ viewport: { width: 1366, height: 768 }, hasTouch: mode.touch })
    const hit = (l: Locator) => (mode.touch ? l.tap() : l.click())
    const shot = async (page: Page, name: string) => {
      if (SHOTS && mode.touch) await page.screenshot({ path: `${SHOTS}/${name}.png` })
    }

    test('«+ ещё строку» — только услуги «прочее» этого чека; строка — в книжку, удалили услугу — строки нет', async ({ page }) => {
      test.setTimeout(150_000)
      await login(page, mode.touch)
      const s = await setup(page.request)
      await page.reload()
      await expect(page.locator('[data-picker]')).toBeFocused()

      const cart = page.getByTestId('cart').getByRole('listitem')
      const scan = async (code: string, lines: number) => {
        await page.locator('[data-picker]').focus()
        await page.keyboard.type(code, { delay: 5 })
        await page.keyboard.press('Enter')
        await expect(cart).toHaveCount(lines)
      }

      // Чек «В сервис»: замена масла, ГУР и кардан; масло, ATF и антифриз — товарами.
      await hit(page.getByRole('button', { name: 'В сервис' }))
      await addKinds(page, [s.service, s.gur, s.shaft], hit)
      await scan(s.oilCode, 4)
      await scan(s.gearCode, 5)
      await scan(s.antiCode, 6)
      await serviceDetails(page, s, mode.touch, hit)
      // Раскладку меряем без временной плашки «снимок каталога устарел»: снимок кассы ещё сохраняется.
      await expect(page.getByText(/снимок каталога устарел/)).toHaveCount(0, { timeout: 30_000 })

      // Строкой встала только замена масла: ATF и антифриз без своих услуг — доливка, ГУР и кардан —
      // «прочее», только через «+ ещё».
      const block = page.getByTestId('next-change')
      await expect(block.getByLabel(`Следующая замена: ${s.service}`)).toHaveValue('225000')
      const fields = block.locator('input[id^="next-"]')
      await expect(fields).toHaveCount(1)
      const more = block.getByRole('button', { name: '+ ещё строку' })
      await expect(more).toBeVisible()
      if (mode.touch) expect((await more.boundingBox())!.height).toBeGreaterThanOrEqual(56)
      await hit(more)
      const dlg = page.getByRole('dialog', { name: 'Следующая замена: ещё строка' })
      const choices = dlg.getByTestId('next-extra-choices').getByRole('button').filter({ hasNotText: 'Отмена' })
      await expect(choices).toHaveText([s.gur, s.shaft])
      // Ни товаров чека, ни справочника услуг.
      for (const no of [/ATF/, /антифриз/i, /Замена масла/, /Промывка/]) await expect(dlg.getByText(no)).toHaveCount(0)
      if (mode.touch) {
        // Крупные кнопки под палец, страница не прокручивается.
        for (const b of await dlg.getByTestId('next-extra-choices').getByRole('button').all()) {
          expect((await b.boundingBox())!.height).toBeGreaterThanOrEqual(56)
        }
        expect(await noScroll(page)).toBe(true)
      }
      await shot(page, 'next-cart-choices-1366')
      await hit(dlg.getByRole('button', { name: s.gur }))
      await expect(dlg).toHaveCount(0)

      // ГУР: интервала у услуги нет — поле пустое; кардан — пробег + интервал услуги (20 000 км).
      const gurField = block.getByLabel(`Следующая замена: ${s.gur}`)
      await expect(gurField).toHaveValue('')
      await hit(more)
      await expect(choices).toHaveText([s.shaft])
      await hit(dlg.getByRole('button', { name: s.shaft }))
      const shaftField = block.getByLabel(`Следующая замена: ${s.shaft}`)
      await expect(shaftField).toHaveValue('237000')
      await expect(more).toHaveCount(0)
      await expect(fields).toHaveCount(3)
      if (mode.touch) {
        // Подпись строки «прочее» — название услуги.
        await expect(block.locator('[data-next-label]').filter({ hasText: s.gur })).toBeVisible()
        await expect(block).toBeInViewport({ ratio: 1 })
        expect(await noScroll(page)).toBe(true)
        await expect(page.getByRole('button', { name: /Провести чек/ })).toBeInViewport()
      }
      await shot(page, 'next-cart-row-1366')

      // Строки переживают перезагрузку (черновик чека).
      await gurField.fill('300000')
      await page.reload()
      await expect(block.getByLabel(`Следующая замена: ${s.gur}`)).toHaveValue('300000')
      await expect(shaftField).toHaveValue('237000')

      // Удалили ГУР из чека — его строки нет.
      await hit(cart.filter({ hasText: s.gur }).getByRole('button', { name: 'Удалить строку' }))
      await expect(cart).toHaveCount(5)
      await expect(gurField).toHaveCount(0)
      await shot(page, 'next-cart-removed-1366')
      if (mode.touch) {
        // «Вернуть» возвращает и строку ГУР с вписанным пробегом.
        await page.waitForTimeout(400)
        await hit(page.getByRole('button', { name: 'Вернуть' }))
        await expect(cart).toHaveCount(6)
        await expect(gurField).toHaveValue('300000')
      } else {
        // Снова в чеке — строка сама не встаёт, «+ ещё» снова предлагает ГУР.
        await hit(page.getByRole('button', { name: 'Изменить вид' }))
        await addKinds(page, [s.gur], hit)
        await expect(cart).toHaveCount(6)
        await expect(gurField).toHaveCount(0)
        await hit(more)
        await hit(dlg.getByRole('button', { name: s.gur }))
        await gurField.fill('300000')
      }

      // Лишний ноль — подсказка «проверьте», провести можно.
      await shaftField.fill('437000')
      await expect(block.getByTestId('next-far')).toContainText(/через 220\s000 км — проверьте/)
      await expect(page.getByText(/Не хватает:.*следующая замена/)).toHaveCount(0)
      await shaftField.fill('237000')
      await expect(block.getByTestId('next-far')).toHaveCount(0)

      await hit(page.getByRole('button', { name: 'Без сдачи' }))
      await hit(page.getByRole('button', { name: /Провести чек/ }).first())
      await expect(page.getByText(/Чек № .+ проведён/)).toBeVisible()

      // Итог чека: следующая замена масла и каждой услуги «прочее» — её названием.
      const written = page.getByText(/Записано в масляную книжку/).locator('..')
      await expect(written).toContainText(/Следующая · Масло двигателя: на 225\s000 км/)
      await expect(written).toContainText(new RegExp(`Следующая · ${s.gur}: на 300\\s000 км`))
      await expect(written).toContainText(new RegExp(`Следующая · ${s.shaft}: на 237\\s000 км`))
      await expect(written).not.toContainText('Антифриз')
      await expect(written).not.toContainText('Масло в коробке')
      await shot(page, 'next-cart-done-1366')

      // Чек книжки после чека: только жидкости — масло; «прочего» нет (ADR-116).
      await expect.poll(async () => (await prints(page)).some((h) => h.includes('Масляная книжка'))).toBe(true)
      const book = plain((await prints(page)).find((h) => h.includes('Масляная книжка')) ?? '')
      expect(book.split('Следующая замена:')[1] ?? '').toContain('Масло двигателя на 225 000 км')
      for (const no of ['ГУР', 'кардан', 'Антифриз']) expect(book).not.toContain(no)

      // Печатный чек: блок «Масляная книжка» — тоже только жидкости.
      await hit(page.getByRole('button', { name: 'Печать чека' }))
      await expect.poll(async () => (await prints(page)).some((h) => h.includes('Товарный чек'))).toBe(true)
      const receipt = plain((await prints(page)).filter((h) => h.includes('Товарный чек')).at(-1) ?? '')
      expect(receipt).toContain('Следующая замена: Масло двигателя: на 225 000 км')
      expect(receipt).not.toContain(`${s.gur}: на`)

      // Книжка машины: строка у каждой услуги «прочее» со своей целью; ни ATF, ни антифриза.
      const b = await api<{ records: { lines: { slot: string; text: string; next_mileage: number | null }[] }[]; slots: { slot: string; label: string; service_id?: string | null; next_mileage?: number | null }[] }>(
        page.request,
        `/vehicles/${s.vehicle.id}/oil-book`,
      )
      const rec = b.records[0].lines
      expect(rec.map((l) => l.slot)).toEqual(['oil', 'other', 'other'])
      expect(rec[0].next_mileage).toBe(225000)
      const others = { [s.gur]: 300000, [s.shaft]: 237000 }
      expect(Object.fromEntries(rec.filter((l) => l.slot === 'other').map((l) => [l.text, l.next_mileage]))).toEqual(others)
      expect(Object.fromEntries(b.slots.filter((x) => x.slot === 'other').map((x) => [x.label, x.next_mileage]))).toEqual(others)
      expect(b.slots.find((x) => x.label === s.gur)?.service_id).toBe(s.gurId)
      expect(b.slots.some((x) => x.slot === 'antifreeze' || x.slot === 'gear_oil')).toBe(false)
      if (mode.touch) await hit(page.getByRole('button', { name: 'Выйти из режима кассы' }))
    })

    test('услуга плиткой «ходовое» (масло клиента) — строка «Следующей замены» по ней и в книжку', async ({ page }) => {
      test.setTimeout(120_000)
      await login(page, mode.touch)
      const s = await setup(page.request)
      await withTiles(
        page.request,
        [
          { kind: 'service', id: s.serviceId },
          { kind: 'service', id: s.antiServiceId },
        ],
        async () => {
          await page.reload()
          const tile = (name: string) => page.getByRole('button', { name: new RegExp(`^${name}`) })
          const cart = page.getByTestId('cart').getByRole('listitem')
          // Замена масла плиткой, товара-масла в чеке нет (масло клиента) — чек сам стал «В сервис».
          await hit(tile(s.service))
          await expect(cart).toHaveCount(1)
          // В обычной кассе плитки видны и у чека «В сервис» — замена антифриза тоже плиткой.
          if (!mode.touch) {
            await hit(tile(s.antiService))
            await expect(cart).toHaveCount(2)
          }
          await serviceDetails(page, s, mode.touch, hit)
          const block = page.getByTestId('next-change')
          await expect(block.getByLabel(`Следующая замена: ${s.service}`)).toHaveValue('225000')
          if (!mode.touch) await expect(block.getByLabel(`Следующая замена: ${s.antiService}`)).toHaveValue('277000')
          await hit(page.getByRole('button', { name: 'Без сдачи' }))
          await hit(page.getByRole('button', { name: /Провести чек/ }).first())
          await expect(page.getByText(/Чек № .+ проведён/)).toBeVisible()
          const written = page.getByText(/Записано в масляную книжку/).locator('..')
          await expect(written).toContainText(/Следующая · Масло двигателя: на 225\s000 км/)
          if (!mode.touch) await expect(written).toContainText(/Следующая · Антифриз: на 277\s000 км/)
        },
      )
      if (mode.touch) await hit(page.getByRole('button', { name: 'Выйти из режима кассы' }))
    })
  })
}

test.describe('режим кассы 1366×768: «+ ещё» под пальцем', () => {
  test.use({ viewport: { width: 1366, height: 768 }, hasTouch: true })
  const hit = (l: Locator) => l.tap()
  const lines = (page: Page) => page.getByTestId('cart').getByRole('listitem')

  /** Скан в поиск товара: строк в чеке стало `count`. */
  const scan = async (page: Page, code: string, count: number) => {
    await page.locator('[data-picker]').focus()
    await page.keyboard.type(code, { delay: 5 })
    await page.keyboard.press('Enter')
    await expect(lines(page)).toHaveCount(count)
  }

  /** Чек «В сервис» с видами замены. */
  const serviceKind = async (page: Page, names: string[]) => {
    await hit(page.getByRole('button', { name: 'В сервис' }))
    await addKinds(page, names, hit)
  }

  test('экранная клавиатура открыта — «+ ещё» её прячет: касание кнопки окна не набирает пробег', async ({ page }) => {
    await login(page, true)
    const s = await setup(page.request)
    await page.reload()
    await serviceKind(page, [s.service, s.gur, s.shaft])
    await serviceDetails(page, s, true, hit)
    // Пробег — экранной клавиатурой по «⌨» (ADR-101): она остаётся открытой и после касания кнопки.
    const mileage = page.locator('#mileage')
    await mileage.tap()
    await hit(page.getByTestId('keyboard-field-button'))
    const pad = page.getByTestId('keypad')
    await expect(pad).toBeVisible()
    await hit(page.getByTestId('next-change').getByRole('button', { name: '+ ещё строку' }))
    const dlg = page.getByRole('dialog', { name: 'Следующая замена: ещё строка' })
    await expect(dlg).toBeVisible()
    await expect(pad).toHaveCount(0)
    await expect(page.getByTestId('keyboard-field-button')).toHaveCount(0)
    // Правый край каждой кнопки окна — сама кнопка, а не клавиша поверх неё.
    for (const b of await dlg.getByRole('button').all()) {
      const box = (await b.boundingBox())!
      const mine = await b.evaluate((el, p) => el.contains(document.elementFromPoint(p.x, p.y)), { x: box.x + box.width - 8, y: box.y + box.height / 2 })
      expect(mine, await b.innerText()).toBe(true)
    }
    await hit(dlg.getByRole('button', { name: s.shaft }))
    await expect(dlg).toHaveCount(0)
    await expect(mileage).toHaveValue('217000')
    await expect(page.getByTestId('next-change').getByLabel(`Следующая замена: ${s.shaft}`)).toHaveValue('237000')
  })

  test('строка, добавленная «+ ещё», видна, даже когда карточка замены прокручивается', async ({ page }) => {
    test.setTimeout(90_000)
    await login(page, true)
    const s = await setup(page.request)
    await page.reload()
    // Отложенный чек — сверху плашка «Отложенные»: карточке замены остаётся меньше места.
    await scan(page, s.oilCode, 1)
    await hit(page.getByRole('button', { name: 'Отложить', exact: true }))
    await expect(lines(page)).toHaveCount(0)
    await serviceKind(page, [s.service, s.gur, s.shaft, s.flush])
    await scan(page, s.oilCode, 5)
    await scan(page, s.gearCode, 6)
    await scan(page, s.fuelCode, 7)
    await serviceDetails(page, s, true, hit)
    const block = page.getByTestId('next-change')
    const card = page.getByTestId('service-left')
    const fields = block.locator('input[id^="next-"]')
    await expect(fields).toHaveCount(1)
    for (let i = 2; i <= 4; i++) {
      await hit(block.getByRole('button', { name: '+ ещё строку' }))
      const dlg = page.getByRole('dialog', { name: 'Следующая замена: ещё строка' })
      await hit(dlg.getByTestId('next-extra-choices').getByRole('button').first())
      await expect(dlg).toHaveCount(0)
      await expect(fields).toHaveCount(i)
      // Новое поле и его «✕» — в видимой части карточки: кассир видит, что касание сработало.
      const added = fields.last()
      const remove = block.getByRole('button', { name: /^Убрать строку/ }).last()
      for (const l of [added, remove]) {
        await expect
          .poll(() =>
            l.evaluate((el, box) => {
              const r = el.getBoundingClientRect()
              const c = (document.querySelector(box) as HTMLElement).getBoundingClientRect()
              return r.top >= c.top - 1 && r.bottom <= c.bottom + 1 && r.bottom <= window.innerHeight
            }, '[data-testid="service-left"]'),
          )
          .toBe(true)
      }
    }
    await expect(card).toBeVisible()
    expect(await noScroll(page)).toBe(true)
  })

  test('список чека стоит внизу и после выбора клиента: последняя строка видна целиком', async ({ page }) => {
    await login(page, true)
    const s = await setup(page.request)
    await page.reload()
    await serviceKind(page, [s.service])
    await scan(page, s.oilCode, 2)
    await scan(page, s.gearCode, 3)
    await scan(page, s.fuelCode, 4)
    await serviceDetails(page, s, true, hit)
    await expect(page.getByTestId('next-change')).toBeVisible()
    const list = page.getByTestId('cart')
    await expect.poll(() => list.evaluate((el) => el.scrollTop + el.clientHeight >= el.scrollHeight - 2)).toBe(true)
    await expect(lines(page).last()).toBeInViewport({ ratio: 1 })
    await expect(page.getByTestId('cart-more').filter({ hasText: '↓' })).toHaveCount(0)
  })

  test('три строки замены и фильтры с маслом: подписи — названием услуги, приписка видна целиком', async ({ page }) => {
    // Проверка 27.10.2026: подписи «Масло двиг…» обрезались, приписка про фильтры уходила за край.
    test.setTimeout(90_000)
    await login(page, true)
    const s = await setup(page.request)
    const gearService = `Частичная замена масла в коробке ещё ${RUN}${++seq}`
    await api(page.request, '/services', { name: gearService, price_tyiyn: 40_000, master_fee_tyiyn: 3_000, book_slot: 'gear_oil' })
    const filters = await api<{ id: string }>(page.request, '/categories', { name: `Фильтры три ${RUN}${seq}`, kind: 'filter' })
    const airCode = `47${RUN}${seq}`
    const air = await api<{ id: string }>(page.request, '/products', { op_id: crypto.randomUUID(), category_id: filters.id, name: `Фильтр воздушный три ${seq}`, attrs: { filter_type: 'воздушный' }, barcodes: [airCode], sale_price_tyiyn: 60_000 })
    await api(page.request, '/receipts', { op_id: crypto.randomUUID(), paid_from: await ownerSafe(page.request), lines: [{ product_id: air.id, qty: 5, cost_tyiyn: 100_000 }] })
    await page.reload()
    await serviceKind(page, [s.service, gearService, s.antiService, s.gur])
    await scan(page, s.oilCode, 5)
    await scan(page, airCode, 6)
    await serviceDetails(page, s, true, hit)
    await expect(page.getByText(/снимок каталога устарел/)).toHaveCount(0, { timeout: 30_000 })
    const block = page.getByTestId('next-change')
    await expect(block.locator('input[id^="next-"]')).toHaveCount(3)
    // Название услуги — целиком или переносом, под ним мелко — строка книжки.
    for (const name of [s.service, gearService, s.antiService]) {
      const label = block.locator('[data-next-label]').filter({ hasText: name })
      await expect(label).toBeInViewport({ ratio: 1 })
    }
    await expect(block.getByText('Масло в коробке', { exact: true })).toBeVisible()
    // Приписка про фильтры — одна строка без двух двоеточий и видна целиком.
    const note = page.getByTestId('next-filters')
    await expect(note).toHaveText('Масло двигателя — и фильтр: воздушный на тот же пробег')
    await expect(note).toBeInViewport({ ratio: 1 })
    await expect(page.getByTestId('service-left').getByRole('button', { name: 'Мастер рекомендовал' })).toBeInViewport({ ratio: 1 })
    expect(await noScroll(page)).toBe(true)
    if (SHOTS) await page.screenshot({ path: `${SHOTS}/next-cart-three-1366.png` })
  })
})
