// Быстрая касса (SPEC-36 §16): окна услуг — сразу из снимка, книжка машины — один запрос, полоса
// связи не читает весь снимок, большой снимок ждёт пустого чека, первая загрузка кассы — без
// экранов владельца, касса без сети открывается со всеми частями сборки.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2fb' }
const RUN = Date.now().toString().slice(-8)

test.use({ viewport: { width: 1366, height: 768 }, hasTouch: true })

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  // Режим кассы: без меню, сразу касса.
  await expect(page.getByTestId('cashier-mode-bar')).toBeVisible()
}

async function apiPost<T>(request: APIRequestContext, path: string, data: unknown): Promise<T> {
  const res = await request.post(`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

interface Product {
  id: string
  name: string
  unit: string
  sale_price_tyiyn: number
}

let seq = 0

/** Товар с остатком — строка черновика чека. */
async function product(request: APIRequestContext): Promise<Product> {
  const cat = await apiPost<{ id: string }>(request, '/categories', { name: `Быстрая ${RUN}${++seq}`, kind: 'other' })
  const p = await apiPost<Product>(request, '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name: `Ароматизатор быстрый ${RUN}${seq}`, sale_price_tyiyn: 40_000 })
  await apiPost(request, '/receipts', { op_id: crypto.randomUUID(), paid_from: await ownerSafe(request), lines: [{ product_id: p.id, qty: 5, cost_tyiyn: 100_000 }] })
  return p
}

/** Чек со строкой (и клиентом с машиной) — в черновик кассы: как после перехода между экранами. */
async function draft(page: Page, p: Product, extra: Record<string, unknown> = {}) {
  // Касса успела загрузиться и сохранить свой пустой черновик — иначе он сотрёт наш.
  await page.waitForSelector('[data-picker]')
  await page.waitForTimeout(800)
  await page.evaluate(
    ([p, extra]) =>
      sessionStorage.setItem(
        'avtodom.cart',
        JSON.stringify({ saleType: 'takeaway', masterId: '', comment: '', lines: [{ key: 'k1', kind: 'piece', product: p, qtyText: '1', priceText: String(p.sale_price_tyiyn / 100) }], ...extra }),
      ),
    [p, extra] as const,
  )
}

/** Снимок сохранён на устройстве. */
async function snapshotSaved(page: Page) {
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          new Promise<boolean>((res) => {
            const r = indexedDB.open('avtodom', 1)
            r.onsuccess = () => {
              const g = r.result.transaction('snapshot').objectStore('snapshot').getKey('meta')
              g.onsuccess = () => res(g.result !== undefined)
              g.onerror = () => res(false)
            }
            r.onerror = () => res(false)
          }),
      ),
    )
    .toBe(true)
}

/** Счётчик чтений и записей всего снимка ('current') в IndexedDB. */
async function countSnapshotIo(page: Page) {
  await page.addInitScript(() => {
    const w = window as unknown as { __snap: { get: number; put: number } }
    w.__snap = { get: 0, put: 0 }
    const get = IDBObjectStore.prototype.get
    IDBObjectStore.prototype.get = function (this: IDBObjectStore, key: IDBValidKey | IDBKeyRange) {
      if (this.name === 'snapshot' && key === 'current') w.__snap.get++
      return get.call(this, key)
    }
    const put = IDBObjectStore.prototype.put
    IDBObjectStore.prototype.put = function (this: IDBObjectStore, value: unknown, key?: IDBValidKey) {
      if (this.name === 'snapshot' && key === 'current') w.__snap.put++
      return put.call(this, value, key)
    }
  })
}

const snapIo = (page: Page) => page.evaluate(() => (window as unknown as { __snap: { get: number; put: number } }).__snap)
const resetSnapIo = (page: Page) => page.evaluate(() => ((window as unknown as { __snap: { get: number; put: number } }).__snap = { get: 0, put: 0 }))

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('avtodom.cashier_mode', '1'))
})

test('«Вид замены» и «Услуги» открываются из снимка, свежий список приходит в фоне', async ({ page }) => {
  await login(page)
  const p = await product(page.request)
  await page.goto('/')
  await snapshotSaved(page)
  // Все запросы снимка при открытии кассы завершены — новая услуга в него не попадёт.
  await page.waitForLoadState('networkidle')
  // Услуга заведена после снимка: её нет в снимке, она придёт со свежим списком.
  const fresh = `Замена масла свежая ${RUN}`
  await apiPost(page.request, '/services', { name: fresh, price_tyiyn: 20_000, master_fee_tyiyn: 3_000 })
  let release: () => void = () => undefined
  const held = new Promise<void>((r) => (release = r))
  await page.route(/\/api\/v1\/services$/, async (route) => {
    await held
    await route.continue()
  })
  await draft(page, p)
  await page.reload()
  await expect(page.getByTestId('cart').getByRole('listitem')).toHaveCount(1)

  // Сервер ещё не ответил, а окно уже со списком.
  await page.getByRole('button', { name: 'В сервис' }).first().tap()
  const kinds = page.getByRole('dialog', { name: 'Вид замены' })
  await expect(kinds.getByTestId('service-choices').getByRole('button').first()).toBeVisible({ timeout: 2_000 })
  await kinds.getByPlaceholder('Найти услугу').fill(fresh)
  await expect(kinds.getByRole('button', { name: new RegExp(fresh) })).toHaveCount(0)
  release()
  await expect(kinds.getByRole('button', { name: new RegExp(fresh) })).toBeVisible()
  await page.keyboard.press('Escape')
  await page.getByRole('button', { name: 'На вынос', exact: true }).first().tap()

  // Следующее окно — сразу со свежим списком, ещё до нового ответа сервера.
  let hold2: () => void = () => undefined
  const held2 = new Promise<void>((r) => (hold2 = r))
  await page.unroute(/\/api\/v1\/services$/)
  await page.route(/\/api\/v1\/services$/, async (route) => {
    await held2
    await route.continue()
  })
  await page.getByRole('button', { name: 'Услуги', exact: true }).tap()
  const services = page.getByRole('dialog', { name: 'Услуги' })
  await services.getByPlaceholder('Найти услугу').fill(fresh)
  await expect(services.getByRole('button', { name: new RegExp(fresh) })).toBeVisible({ timeout: 2_000 })
  hold2()
})

test('книжка машины — один запрос: подсказка в окне клиента её не перезапрашивает', async ({ page }) => {
  await login(page)
  const p = await product(page.request)
  const party = await apiPost<{ id: string }>(page.request, '/parties', { kind: 'person', first_name: 'Книжка', last_name: `Один${RUN}`, phone: `+99677${RUN.slice(-7)}` })
  const plate = `01KG${RUN.slice(-3)}BK`
  const car = await apiPost<{ id: string }>(page.request, `/parties/${party.id}/vehicles`, { plate })
  const full = await (await page.request.get(`/api/v1/parties/${party.id}`)).json()
  const books: string[] = []
  page.on('request', (r) => {
    if (r.url().includes('/oil-book')) books.push(r.url())
  })
  await page.goto('/')
  await draft(page, p, { party: full, vehicleId: car.id })
  await page.reload()
  await expect(page.getByTestId('cart').getByRole('listitem')).toHaveCount(1)
  await expect.poll(() => books.length).toBe(1)
  await page.locator('[data-client]').first().tap()
  const dlg = page.getByRole('dialog', { name: 'Клиент и машина' })
  await expect(dlg.getByText('В масляной книжке этой машины пока пусто.')).toBeVisible()
  await page.waitForTimeout(800)
  expect(books).toHaveLength(1)
})

test('полоса связи не читает снимок; новый снимок при непустом чеке ждёт пустого', async ({ page }) => {
  await countSnapshotIo(page)
  await login(page)
  const p = await product(page.request)
  await page.goto('/')
  await snapshotSaved(page)
  await page.waitForTimeout(1_000)

  // «Снимок сохранён» и возврат связи (304): весь снимок не читается и не перезаписывается.
  await resetSnapIo(page)
  await page.evaluate(() => window.dispatchEvent(new Event('avtodom:snapshot')))
  const answered = page.waitForResponse((r) => r.url().endsWith('/api/v1/offline/snapshot'))
  await page.evaluate(() => window.dispatchEvent(new Event('online')))
  expect((await answered).status()).toBe(304)
  await page.waitForTimeout(800)
  expect(await snapIo(page)).toEqual({ get: 0, put: 0 })

  // Снимок на сервере сменился, а в чеке строка: большой снимок не сохраняется.
  await draft(page, p)
  // Снимок, запрошенный при открытии кассы, уже проверен — дальше его время не перепишет нашу версию.
  const startup = page.waitForResponse((r) => r.url().endsWith('/api/v1/offline/snapshot'))
  await page.reload()
  await startup
  await expect(page.getByTestId('cart').getByRole('listitem')).toHaveCount(1)
  await page.waitForTimeout(500)
  await page.evaluate(
    () =>
      new Promise<void>((res) => {
        const r = indexedDB.open('avtodom', 1)
        r.onsuccess = () => {
          const tx = r.result.transaction('snapshot', 'readwrite')
          const store = tx.objectStore('snapshot')
          const g = store.get('meta')
          g.onsuccess = () => store.put({ ...g.result, version: 'old' }, 'meta')
          tx.oncomplete = () => res()
        }
      }),
  )
  await page.waitForTimeout(1_000)
  await resetSnapIo(page)
  const full = page.waitForResponse((r) => r.url().endsWith('/api/v1/offline/snapshot'))
  await page.evaluate(() => window.dispatchEvent(new Event('online')))
  expect((await full).status()).toBe(200)
  await page.waitForTimeout(1_000)
  expect((await snapIo(page)).put).toBe(0)

  // Чек опустел — снимок сохраняется.
  await page.getByRole('button', { name: 'Очистить', exact: true }).tap()
  await page.getByRole('button', { name: 'Точно очистить?' }).tap()
  await expect(page.getByTestId('cart').getByRole('listitem')).toHaveCount(0)
  await page.evaluate(() => window.dispatchEvent(new Event('online')))
  await expect.poll(async () => (await snapIo(page)).put).toBe(1)
})

test('касса грузится без экранов владельца, без сети открывается со всеми частями', async ({ page, context }) => {
  await login(page)
  const scripts: string[] = []
  page.on('response', (r) => {
    const u = new URL(r.url())
    if (u.pathname.startsWith('/assets/') && u.pathname.endsWith('.js')) scripts.push(u.pathname)
  })
  await page.goto('/')
  await page.waitForSelector('[data-picker]')
  await page.evaluate(async () => {
    await navigator.serviceWorker.ready
  })
  await page.waitForTimeout(1_500)
  // Экраны владельца («Сводка», «Товары», «Расчёт», «Приход») — отдельными частями, касса без них.
  expect(scripts.length).toBeGreaterThan(0)
  const owner = /\/(Owner|Products|Payroll|Receipts|ReceiptNew|Revision)-/
  expect(scripts.filter((s) => owner.test(s))).toEqual([])
  // «Сводка» владельца подгружает свою часть только при переходе на неё.
  const atCashier = scripts.length
  await page.goto('/summary')
  await expect.poll(() => scripts.slice(atCashier).some((s) => /\/Owner-/.test(s))).toBe(true)
  await page.goto('/')
  await page.waitForSelector('[data-picker]')

  // Без сети: касса открывается после перезагрузки, другие экраны — из кэша Service Worker.
  await context.setOffline(true)
  try {
    await page.reload()
    await page.waitForSelector('[data-picker]')
    for (const [path, title] of [
      ['/sales', 'Чеки за день'],
      ['/settings', 'Настройки'],
    ]) {
      await page.goto(path)
      await expect(page.getByRole('heading', { name: title }).first()).toBeVisible()
      await expect(page.getByText('Экран не загрузился')).toHaveCount(0)
    }
  } finally {
    await context.setOffline(false)
  }
})
