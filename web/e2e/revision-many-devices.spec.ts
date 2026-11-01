// Один товар лежит в нескольких местах, его сканируют разные люди с разных устройств в одной
// ревизии — итог складывается и виден всем (задача 003, Н14).
import { expect, test, type APIRequestContext, type Browser, type BrowserContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const RUN = Date.now().toString().slice(-8)
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2f1' }
/** Текст с неразрывными пробелами: «2 кан. + 1,5 л» — как на экране. */
const text = (t: string) => new RegExp(t.replace(/[.*+?^${}()|[\]\\]/g, '\\$&').replace(/ /g, '\\s'))

/** Отдельное устройство: свой контекст браузера, свой вход. */
async function device(browser: Browser): Promise<Page> {
  const page = await (await browser.newContext()).newPage()
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
  return page
}

async function apiPost<T>(request: APIRequestContext, path: string, data: unknown): Promise<T> {
  const res = await request.post(`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

async function openRevision(page: Page, category: string) {
  await page.goto('/revision')
  await page.getByRole('row', { name: new RegExp(category) }).first().click()
  await expect(page.getByRole('heading', { name: new RegExp(`Ревизия № \\d+ · ${category}`) })).toBeVisible()
}

/** Черновик категории не оставляем: «Весь склад» в других проверках получил бы «Идёт ревизия». */
async function cancelDraft(request: APIRequestContext, categoryId: string) {
  const list = (await (await request.get('/api/v1/revisions')).json()) as { id: string; status: string; category_id: string | null }[]
  for (const r of list.filter((x) => x.status === 'draft' && x.category_id === categoryId)) await apiPost(request, `/revisions/${r.id}/cancel`, {})
}

/** Общий итог строки на экране устройства. */
const total = (page: Page, id: string) => page.locator(`[data-total="${id}"]`)

async function scan(page: Page, code: string) {
  const scanner = page.getByPlaceholder('Сканируйте штрихкод')
  await scanner.fill(code)
  await scanner.press('Enter')
}

test('ревизия: два устройства сканируют один товар — у обоих итог 2', async ({ browser }) => {
  const a = await device(browser)
  const b = await device(browser)
  const category = `Кат два устройства ${RUN}`
  const code = `27${RUN}401`
  const cat = await apiPost<{ id: string }>(a.request, '/categories', { name: category, kind: 'other' })
  const p = await apiPost<{ id: string }>(a.request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Свеча два устройства ${RUN}`,
    barcodes: [code],
    sale_price_tyiyn: 30_000,
  })
  await apiPost(a.request, '/revisions', { category_id: cat.id })
  await openRevision(a, category)
  await openRevision(b, category)

  // Оба открыли строку с пустым пересчётом; каждый сканирует по разу.
  await Promise.all([scan(a, code), scan(b, code)])
  // Свой скан виден сразу, чужой — при перечитывании (раз в 5 с): у обоих 2, а не 1.
  for (const page of [a, b]) {
    await expect(total(page, p.id)).toHaveText(text('итого 2 шт'), { timeout: 15_000 })
    // Кто сколько добавил (задача 005, Н7): оба устройства — один вход, прибавки сложены.
    await expect(page.getByTestId('counted-by').first()).toContainText(text('+2 шт = 2 шт'), { timeout: 15_000 })
  }

  // Ещё скан на первом — второй увидит 3.
  await scan(a, code)
  await expect(total(a, p.id)).toHaveText(text('итого 3 шт'))
  await expect(total(b, p.id)).toHaveText(text('итого 3 шт'), { timeout: 15_000 })

  // Строку уже считали — поле второго «+ добавить»: он нашёл 2 в своём месте — у обоих 5.
  const rowB = b.getByRole('row', { name: new RegExp(`Свеча два устройства ${RUN}`) })
  await expect(rowB.locator('[data-mode="add"]')).toHaveAttribute('aria-pressed', 'true')
  const field = b.locator(`[data-count="${p.id}"]`)
  await expect(field).toHaveValue('')
  await field.fill('2')
  await field.press('Enter')
  await expect(total(b, p.id)).toHaveText(text('итого 5 шт'))
  await expect(field).toHaveValue('')
  await expect(total(a, p.id)).toHaveText(text('итого 5 шт'), { timeout: 15_000 })

  // «= итог»: второй начал вписывать итог 10, а первый тем временем отсканировал ещё — итог не
  // затирается молча: «сейчас 6 шт. Записать итог 10 шт?».
  await rowB.locator('[data-mode="set"]').click()
  await expect(field).toHaveValue('5')
  await field.fill('10')
  await scan(a, code)
  await expect(total(a, p.id)).toHaveText(text('итого 6 шт'))
  await field.press('Enter')
  await expect(b.getByTestId('count-changed')).toContainText(text('сейчас 6 шт'))
  await b.getByRole('button', { name: text('Записать итог 10 шт') }).click()
  await expect(total(b, p.id)).toHaveText(text('итого 10 шт'))
  await expect(total(a, p.id)).toHaveText(text('итого 10 шт'), { timeout: 15_000 })
  // Кто сколько посчитал — в строке.
  await expect(b.getByTestId('counted-by').first()).toContainText(text('= 10 шт'), { timeout: 15_000 })
  await cancelDraft(a.request, cat.id)
})

test('ревизия: обрыв связи после того, как сервер записал, и повторное нажатие — итог не задвоился', async ({ browser }) => {
  const a = await device(browser)
  const category = `Кат обрыв ${RUN}`
  const code = `27${RUN}405`
  const cat = await apiPost<{ id: string }>(a.request, '/categories', { name: category, kind: 'other' })
  const p = await apiPost<{ id: string }>(a.request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Свеча обрыв ${RUN}`,
    barcodes: [code],
    sale_price_tyiyn: 30_000,
  })
  const rev = await apiPost<{ head: { id: string } }>(a.request, '/revisions', { category_id: cat.id })
  const counted = async () =>
    ((await (await a.request.get(`/api/v1/revisions/${rev.head.id}/lines/${p.id}`)).json()) as { counted_qty: number | null }).counted_qty
  await openRevision(a, category)
  await scan(a, code)
  await expect(total(a, p.id)).toHaveText(text('итого 1 шт'))

  // Сервер принимает запись, а ответ до планшета не доходит (обрыв после записи): `drops` первых
  // записей строки обрываются так, остальные проходят.
  const lines = /\/api\/v1\/revisions\/[0-9a-f-]+\/lines$/
  let drops = 0
  let reached = 0
  await a.route(lines, async (route) => {
    if (route.request().method() !== 'PUT') return route.continue()
    reached++
    if (drops > 0) {
      drops--
      await route.fetch()
      return route.abort('connectionreset')
    }
    return route.continue()
  })

  // Скан: первый ответ потерян — экран сам повторяет тот же запрос с тем же op_id, +1 один раз.
  drops = 1
  await scan(a, code)
  await expect(total(a, p.id)).toHaveText(text('итого 2 шт'), { timeout: 10_000 })
  expect(reached).toBe(2)
  expect(await counted()).toBe(2)

  // «+ добавить 3»: все три попытки дошли до сервера, ответы потеряны — на экране «нет связи»,
  // в поле остаётся 3.
  const field = a.locator(`[data-count="${p.id}"]`)
  drops = 3
  reached = 0
  await field.fill('3')
  await field.press('Enter')
  await expect(a.getByText('Нет связи с сервером', { exact: true })).toBeVisible({ timeout: 10_000 })
  expect(reached).toBe(3)
  await expect(field).toHaveValue('3')
  expect(await counted()).toBe(5)
  // Связь вернулась, человек нажимает ещё раз — тот же ввод уходит с тем же op_id: 5, а не 8.
  await field.focus()
  await field.press('Enter')
  await expect(total(a, p.id)).toHaveText(text('итого 5 шт'))
  await expect(field).toHaveValue('')
  expect(await counted()).toBe(5)
  // Новый ввод того же числа — новая прибавка.
  await field.fill('3')
  await field.press('Enter')
  await expect(total(a, p.id)).toHaveText(text('итого 8 шт'))
  expect(await counted()).toBe(8)

  // «= итог 20», а пока вводили, другой прибавил 1: «Прибавить 20 — будет 29». Все ответы
  // потеряны — поле переходит в «+ добавить» с тем же числом; Enter уходит с тем же op_id: 29, а не 49.
  const row = a.getByRole('row', { name: new RegExp(`Свеча обрыв ${RUN}`) })
  await row.locator('[data-mode="set"]').click()
  await expect(field).toHaveValue('8')
  await field.fill('20')
  const other = await a.request.put(`/api/v1/revisions/${rev.head.id}/lines`, {
    data: { product_id: p.id, counted_qty: 1, mode: 'add', op_id: crypto.randomUUID() },
    headers: { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2f2' },
  })
  expect(other.ok()).toBeTruthy()
  await field.press('Enter')
  await expect(a.getByTestId('count-changed')).toContainText(text('сейчас 9 шт'))
  drops = 3
  reached = 0
  await a.getByRole('button', { name: text('Прибавить 20 шт — будет 29 шт') }).click()
  await expect(a.getByText('Нет связи с сервером', { exact: true })).toBeVisible({ timeout: 10_000 })
  expect(reached).toBe(3)
  expect(await counted()).toBe(29)
  await expect(row.locator('[data-mode="add"]')).toHaveAttribute('aria-pressed', 'true')
  await expect(field).toHaveValue('20')
  await field.focus()
  await field.press('Enter')
  await expect(total(a, p.id)).toHaveText(text('итого 29 шт'))
  await expect(field).toHaveValue('')
  expect(await counted()).toBe(29)
  await a.unroute(lines)
  await cancelDraft(a.request, cat.id)
})

test('ревизия: связь пропала на 3 скана — отметка ⏳, связь вернулась — итог +3 ровно', async ({ browser }) => {
  const a = await device(browser)
  const category = `Кат без связи ${RUN}`
  const code = `27${RUN}406`
  const cat = await apiPost<{ id: string }>(a.request, '/categories', { name: category, kind: 'other' })
  const p = await apiPost<{ id: string }>(a.request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Свеча без связи ${RUN}`,
    barcodes: [code],
    sale_price_tyiyn: 30_000,
  })
  const rev = await apiPost<{ head: { id: string } }>(a.request, '/revisions', { category_id: cat.id })
  const counted = async () =>
    ((await (await a.request.get(`/api/v1/revisions/${rev.head.id}/lines/${p.id}`)).json()) as { counted_qty: number | null }).counted_qty
  await openRevision(a, category)
  await scan(a, code)
  await expect(total(a, p.id)).toHaveText(text('итого 1 шт'))

  // Wi-Fi на складе пропал: первый скан доходит до сервера, но ответы теряются (и на повторы);
  // два следующих не доходят вовсе.
  const lines = /\/api\/v1\/revisions\/[0-9a-f-]+\/lines$/
  let offline = true
  let lostOp: string | null = null
  await a.route(lines, async (route) => {
    if (route.request().method() !== 'PUT' || !offline) return route.continue()
    const op = (route.request().postDataJSON() as { op_id: string }).op_id
    lostOp ??= op
    if (op === lostOp) {
      await route.fetch()
      return route.abort('connectionreset')
    }
    return route.abort('connectionrefused')
  })
  for (let i = 0; i < 3; i++) await scan(a, code)
  // Ни один скан не пропал молча: у строки — отметка, вверху — сколько не отправлено.
  const banner = a.getByTestId('unsent')
  await expect(banner).toContainText('Не отправлено: 3 — отправится при связи', { timeout: 20_000 })
  const mark = a.locator(`[data-unsent="${p.id}"]`)
  await expect(mark).toHaveText(text('⏳ +3 шт не отправлено'))
  expect(await counted()).toBe(2)

  // Перезагрузка страницы — очередь на месте (localStorage по ревизии).
  await openRevision(a, category)
  await expect(banner).toContainText('Не отправлено: 3')
  await expect(mark).toHaveText(text('⏳ +3 шт не отправлено'))
  // Проводить с неотправленными сканами — предупреждение.
  await a.getByRole('button', { name: 'Провести' }).click()
  await expect(a.getByTestId('unsent-on-post')).toContainText('не отправлено сканов: 3')
  await a.getByRole('button', { name: 'Отмена' }).click()

  // Связь вернулась — очередь уходит сама с теми же op_id: дошедший без ответа не задвоился.
  offline = false
  await expect(banner).toBeHidden({ timeout: 20_000 })
  await expect(mark).toBeHidden()
  await expect(total(a, p.id)).toHaveText(text('итого 4 шт'))
  expect(await counted()).toBe(4)
  await a.unroute(lines)
  await cancelDraft(a.request, cat.id)
})

test('ревизия: незнакомый код завёл другой — второй прибавляет к его товару', async ({ browser }) => {
  const a = await device(browser)
  const b = await device(browser)
  const category = `Кат новый код ${RUN}`
  const code = `27${RUN}402`
  const cat = await apiPost<{ id: string }>(a.request, '/categories', { name: category, kind: 'other' })
  await apiPost(a.request, '/revisions', { category_id: cat.id })
  await openRevision(a, category)
  await openRevision(b, category)

  // Оба сканируют код, которого нет в программе: у обоих — окно новой карточки.
  for (const page of [a, b]) {
    await scan(page, code)
    await expect(page.getByRole('heading', { name: 'Товар, которого нет в программе' })).toBeVisible()
    await page.locator('#revnew-name').fill(`Щётка новый код ${RUN}`)
    await page.locator('#revnew-price').fill('100')
    await page.locator('#revnew-count').fill('1')
  }
  // Первый заводит товар; второй сохраняет следом — товар уже есть, его найденное прибавляется.
  await a.getByRole('button', { name: /Завести/ }).click()
  await expect(a.getByText(/Заведён и посчитан/)).toBeVisible()
  await b.getByRole('button', { name: /Завести/ }).click()
  await expect(b.getByText('Этот товар только что завёл другой — добавили к нему')).toBeVisible()

  const rows = await a.request.get(`/api/v1/products?q=${encodeURIComponent(`Щётка новый код ${RUN}`)}`)
  const found = (await rows.json()) as { id: string }[]
  expect(found).toHaveLength(1)
  for (const page of [a, b]) {
    await expect(total(page, found[0].id)).toHaveText(text('итого 2 шт'), { timeout: 15_000 })
  }
  await cancelDraft(a.request, cat.id)
})

test('ревизия масла: один сканирует канистры, другой вписывает вскрытую литрами — итог сложился', async ({ browser }) => {
  const a = await device(browser)
  const b = await device(browser)
  const category = `Кат масло два устройства ${RUN}`
  const code = `27${RUN}403`
  const cat = await apiPost<{ id: string }>(a.request, '/categories', { name: category, kind: 'oil' })
  const p = await apiPost<{ id: string }>(a.request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Масло два устройства ${RUN} 5W-30`,
    container_ml: 4000,
    barcodes: [code],
    sale_price_tyiyn: 250_000,
  })
  await apiPost(a.request, '/revisions', { category_id: cat.id })
  await openRevision(a, category)
  await openRevision(b, category)

  // Второй открыл строку раньше, чем первый что-то отсканировал: у масла в таре поле — прибавка.
  const rowB = b.getByRole('row', { name: new RegExp(`Масло два устройства ${RUN}`) })
  await expect(rowB.locator('[data-mode="add"]')).toHaveAttribute('aria-pressed', 'true')
  const liters = b.locator(`[data-count-l="${p.id}"]`)
  await liters.fill('1,5')
  // Пока второй вписывает литры, первый сканирует две канистры.
  await scan(a, code)
  await scan(a, code)
  await expect(total(a, p.id)).toHaveText(text('итого 2 кан.'))
  await b.getByPlaceholder('Сканируйте штрихкод').focus()
  for (const page of [a, b]) await expect(total(page, p.id)).toHaveText(text('итого 2 кан. + 1,5 л'), { timeout: 15_000 })
  const list = await (await a.request.get('/api/v1/revisions')).json()
  const rev = (list as { id: string; status: string; category_id: string | null }[]).find((r) => r.status === 'draft' && r.category_id === cat.id)!
  const line = await (await a.request.get(`/api/v1/revisions/${rev.id}/lines/${p.id}`)).json()
  expect(line.counted_qty).toBe(9500)
  await cancelDraft(a.request, cat.id)
})

test('ревизия: двое начинают одну ревизию — один черновик; «весь склад» при открытой категории — присоединиться', async ({ browser }) => {
  const a = await device(browser)
  const b = await device(browser)
  const category = `Кат одна ревизия ${RUN}`
  const cat = await apiPost<{ id: string }>(a.request, '/categories', { name: category, kind: 'other' })
  for (const page of [a, b]) {
    await page.goto('/revision')
    await page.getByLabel('Что пересчитываем').selectOption({ label: category })
  }
  await Promise.all([a, b].map((page) => page.getByRole('button', { name: 'Начать ревизию' }).click()))
  const heading = (page: Page) => page.getByRole('heading', { name: new RegExp(`Ревизия № \\d+ · ${category}`) })
  await expect(heading(a)).toBeVisible()
  await expect(heading(b)).toBeVisible()
  expect(await heading(a).textContent()).toBe(await heading(b).textContent())

  // Третий начинает «Весь склад»: открыта ревизия категории — предлагают присоединиться.
  const c = await device(browser)
  await c.goto('/revision')
  await c.getByLabel('Что пересчитываем').selectOption({ label: 'Весь склад' })
  await c.getByRole('button', { name: 'Начать ревизию' }).click()
  await expect(c.getByTestId('revision-busy')).toContainText('Идёт ревизия')
  await c.getByRole('button', { name: 'Присоединиться' }).click()
  await expect(c.getByRole('heading', { name: /Ревизия № \d+ · / })).toBeVisible()

  await cancelDraft(a.request, cat.id)
})

/** Одно устройство, несколько вкладок: общий контекст браузера (общий localStorage), один вход. */
async function tabsDevice(browser: Browser): Promise<{ ctx: BrowserContext; page: Page }> {
  const ctx = await browser.newContext()
  const page = await ctx.newPage()
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
  return { ctx, page }
}

/** Категория, штучный товар и черновик ревизии; `counted` — итог строки на сервере. */
async function piece(request: APIRequestContext, tag: string, n: string) {
  const category = `Кат ${tag} ${RUN}`
  const code = `27${RUN}${n}`
  const cat = await apiPost<{ id: string }>(request, '/categories', { name: category, kind: 'other' })
  const p = await apiPost<{ id: string }>(request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Свеча ${tag} ${RUN}`,
    barcodes: [code],
    sale_price_tyiyn: 30_000,
  })
  const rev = await apiPost<{ head: { id: string; number: number } }>(request, '/revisions', { category_id: cat.id })
  const counted = async () =>
    ((await (await request.get(`/api/v1/revisions/${rev.head.id}/lines/${p.id}`)).json()) as { counted_qty: number | null }).counted_qty
  return { category, code, cat, p, rev, counted }
}

const LINES = /\/api\/v1\/revisions\/[0-9a-f-]+\/lines$/

test('ревизия: две вкладки одного устройства без связи — оба скана дошли', async ({ browser }) => {
  const { ctx, page: tab1 } = await tabsDevice(browser)
  const { category, code, cat, p, counted } = await piece(tab1.request, 'две вкладки', '501')
  const tab2 = await ctx.newPage()
  await openRevision(tab1, category)
  await openRevision(tab2, category)
  let offline = true
  await ctx.route(LINES, (r) => (r.request().method() === 'PUT' && offline ? r.abort('connectionrefused') : r.continue()))

  // Скан в каждой вкладке: очередь общая — запись одной вкладки не затирает скан другой.
  await scan(tab1, code)
  await expect(tab1.getByTestId('unsent')).toContainText('Не отправлено: 1', { timeout: 20_000 })
  await tab2.bringToFront()
  await scan(tab2, code)
  await expect(tab2.getByTestId('unsent')).toContainText('Не отправлено: 2', { timeout: 20_000 })
  // Первая вкладка видит отметку и скана второй.
  await expect(tab1.getByTestId('unsent')).toContainText('Не отправлено: 2')
  await expect(tab1.locator(`[data-unsent="${p.id}"]`)).toHaveText(text('⏳ +2 шт не отправлено'))

  // Первую закрыли, вторую перезагрузили — в очереди оба.
  await tab1.close()
  await openRevision(tab2, category)
  await expect(tab2.getByTestId('unsent')).toContainText('Не отправлено: 2')
  offline = false
  await expect(tab2.getByTestId('unsent')).toBeHidden({ timeout: 20_000 })
  await expect(total(tab2, p.id)).toHaveText(text('итого 2 шт'))
  expect(await counted()).toBe(2)
  await cancelDraft(tab2.request, cat.id)
})

test('ревизия: «= итог» при неотправленном скане строки ждёт его — лишнего не прибавилось', async ({ browser }) => {
  const a = await device(browser)
  const { category, code, cat, p, counted } = await piece(a.request, 'итог после очереди', '502')
  await openRevision(a, category)
  await scan(a, code)
  await expect(total(a, p.id)).toHaveText(text('итого 1 шт'))

  // Следующий скан не доходит: у строки «⏳ +1».
  let blocked: string | null = null
  let block = true
  await a.route(LINES, (r) => {
    if (r.request().method() !== 'PUT') return r.continue()
    const body = r.request().postDataJSON() as { op_id: string; mode: string }
    if (body.mode === 'add') blocked ??= body.op_id
    return block && body.op_id === blocked ? r.abort('connectionrefused') : r.continue()
  })
  await scan(a, code)
  const mark = a.locator(`[data-unsent="${p.id}"]`)
  await expect(mark).toHaveText(text('⏳ +1 шт не отправлено'), { timeout: 20_000 })

  // Человек видит 1 и ⏳ +1 и пишет «= итог 2»: без связи итог не уходит раньше скана.
  const row = a.getByRole('row', { name: new RegExp(`Свеча итог после очереди ${RUN}`) })
  await row.locator('[data-mode="set"]').click()
  const field = a.locator(`[data-count="${p.id}"]`)
  await field.fill('2')
  await field.press('Enter')
  await expect(a.getByText(/Сначала отправится 1 неотправленный скан этой строки/)).toBeVisible({ timeout: 10_000 })
  await expect(field).toHaveValue('2')
  expect(await counted()).toBe(1)

  // Связь вернулась: скан, затем итог — 2, а не 3.
  block = false
  await field.focus()
  await field.press('Enter')
  await expect(mark).toBeHidden({ timeout: 20_000 })
  await expect(total(a, p.id)).toHaveText(text('итого 2 шт'))
  await expect(field).toHaveValue('2')
  expect(await counted()).toBe(2)
  // Очередь пуста: позже ничего не прибавится.
  await a.waitForTimeout(6_000)
  expect(await counted()).toBe(2)
  await a.unroute(LINES)
  await cancelDraft(a.request, cat.id)
})

test('ревизия: товар не из загруженного списка отсканирован без связи — дошёл при связи', async ({ browser }) => {
  const a = await device(browser)
  const category = `Кат код без связи ${RUN}`
  const code = `27${RUN}503`
  const unknown = `27${RUN}504`
  const cat = await apiPost<{ id: string }>(a.request, '/categories', { name: category, kind: 'other' })
  const rev = await apiPost<{ head: { id: string } }>(a.request, '/revisions', { category_id: cat.id })
  await openRevision(a, category)
  // Товар заведён, когда список ревизии уже загружен (с нулевым остатком, только что заведён).
  const p = await apiPost<{ id: string }>(a.request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Свеча код без связи ${RUN}`,
    barcodes: [code],
    sale_price_tyiyn: 30_000,
  })
  let offline = true
  await a.context().route(/\/api\/v1\//, (r) => (offline ? r.abort('connectionrefused') : r.continue()))
  await scan(a, code)
  await expect(a.locator(`[data-unsent-code="${code}"]`)).toHaveText(text(`⏳ скан ${code} — товар уточнится при связи`), { timeout: 20_000 })
  await scan(a, unknown)
  await expect(a.locator(`[data-unsent-code="${unknown}"]`)).toBeVisible({ timeout: 20_000 })
  await expect(a.getByTestId('unsent')).toContainText('Не отправлено: 2')

  // Связь вернулась: товар нашёлся по коду — строка с итогом 1; кода нет в программе — сказано.
  offline = false
  await expect(a.getByTestId('unsent')).toBeHidden({ timeout: 20_000 })
  await expect(total(a, p.id)).toHaveText(text('итого 1 шт'))
  await expect(a.getByTestId('unsent-lost')).toContainText(`товар не найден по кодам: ${unknown}`)
  const line = await (await a.request.get(`/api/v1/revisions/${rev.head.id}/lines/${p.id}`)).json()
  expect(line.counted_qty).toBe(1)
  await cancelDraft(a.request, cat.id)
})

test('ревизию провели с другого устройства, а сканы ждали связи — сказано, сколько не учтено', async ({ browser }) => {
  const a = await device(browser)
  const { category, code, p, rev } = await piece(a.request, 'проведена без связи', '505')
  await openRevision(a, category)
  await scan(a, code)
  await expect(total(a, p.id)).toHaveText(text('итого 1 шт'))
  let offline = true
  await a.context().route(/\/api\/v1\//, (r) => (offline ? r.abort('connectionrefused') : r.continue()))
  await scan(a, code)
  await scan(a, code)
  await expect(a.getByTestId('unsent')).toContainText('Не отправлено: 2', { timeout: 20_000 })
  await apiPost(a.request, `/revisions/${rev.head.id}/post`, { op_id: crypto.randomUUID(), comment: 'e2e', force: true })
  offline = false
  await expect(a.getByTestId('unsent-closed')).toContainText(`2 скана не учтены: ревизия № ${rev.head.number} проведена`, { timeout: 20_000 })
  await a.getByRole('button', { name: 'Понятно' }).click()
  await expect(a.getByTestId('unsent-closed')).toBeHidden()
})
