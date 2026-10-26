// Коды из старой базы Умаг — везде (SPEC-35, ADR-110). Код Умага связанного товара — код самого
// товара: его находят касса (и в режиме кассы), приход, «Товары». Строки Умага видны только в
// «Товарах» и «Приходе» (и ревизии, «Сканере товаров»): окно «Товар из старой базы Умаг» с
// «Завести» и «Это уже есть»; на кассе Умага нет — незаведённый код там «Код не найден».
// Экран «Связать с Умагом»: сводка, автосвязывание, похожие пары, «Не то», «Отвязать», конфликт кода.
import { expect, test, type APIRequestContext, type Browser, type Page } from '@playwright/test'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e35a' }
const RUN = Date.now().toString().slice(-8)
const PICKER = /Сканируйте штрихкод/
const NO_RIGHT = 'Товар есть в базе Умаг, но не заведён: попросите владельца или администратора завести его'
const money = (text: string) => new RegExp(text.replace(/ /g, '\\s'))
// Снимки экрана для проверки раскладки: только если задан каталог.
const SHOTS = process.env.E2E_SHOTS_DIR

/** Снимок при моноблоке кассы 1366×768 (размер окна меняется только для снимка). */
async function shot1366(page: Page, name: string) {
  if (!SHOTS) return
  await page.setViewportSize({ width: 1366, height: 768 })
  await page.screenshot({ path: `${SHOTS}/${name}.png` })
}

interface Hit {
  id: string
  name: string
  exact: boolean
  product_id: string | null
  product_name: string | null
  codes: string[]
  conflicts: { code: string; product_id: string; product_name: string }[]
}
interface Product {
  id: string
  name: string
  barcodes: string[]
}
interface UmagRow {
  name: string
  barcode: string
  article?: string
  extra_code?: string
  cost_tyiyn?: number
  price_tyiyn?: number
  unit?: string
  supplier?: string
  category?: string
}

async function login(page: Page, user = OWNER, password = OWNER_PASSWORD) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(user)
  await page.getByLabel('Пароль').fill(password)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' }).first()).toBeVisible()
}

async function apiPost<T>(request: APIRequestContext, path: string, data: unknown): Promise<T> {
  const res = await request.post(`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

async function apiGet<T>(request: APIRequestContext, path: string): Promise<T> {
  const res = await request.get(`/api/v1${path}`)
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

/** Новая выгрузка Умага: заменяет список филиала целиком. */
async function putUmag(request: APIRequestContext, rows: UmagRow[]) {
  const res = await request.put('/api/v1/old-catalog', { data: rows, headers: DEVICE })
  expect(res.ok(), await res.text()).toBeTruthy()
}

/** Строка Умага с этим кодом. */
async function umagRow(request: APIRequestContext, code: string): Promise<Hit> {
  const hits = await apiGet<Hit[]>(request, `/old-catalog/search?barcode=${encodeURIComponent(code)}`)
  const hit = hits.find((h) => h.exact)
  expect(hit, `в Умаге нет кода ${code}`).toBeTruthy()
  return hit!
}

async function category(request: APIRequestContext, name: string): Promise<string> {
  return (await apiPost<{ id: string }>(request, '/categories', { name, kind: 'other' })).id
}

async function product(request: APIRequestContext, categoryId: string, name: string, codes: string[], price: number): Promise<Product> {
  return apiPost<Product>(request, '/products', { op_id: crypto.randomUUID(), category_id: categoryId, name, barcodes: codes, sale_price_tyiyn: price })
}

async function stock(request: APIRequestContext, productId: string, qty: number, cost: number) {
  await apiPost(request, '/receipts', { op_id: crypto.randomUUID(), paid_from: await ownerSafe(request), lines: [{ product_id: productId, qty, cost_tyiyn: qty * cost }] })
}

/** Администратор с выбранными правами; владелец уже вошёл на `page`. */
async function adminWith(page: Page, browser: Browser, permissions: string[]) {
  const login_ = `umag${RUN}${Math.random().toString(36).slice(2, 6)}`
  const password = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  const user = await apiPost<{ id: string }>(page.request, '/users', { login: login_, password, full_name: `Админ Умаг ${RUN}`, role: 'admin' })
  const res = await page.request.put(`/api/v1/users/${user.id}/permissions`, { data: { op_id: crypto.randomUUID(), permissions }, headers: DEVICE })
  expect(res.ok(), await res.text()).toBeTruthy()
  const ctx = await browser.newContext()
  const a = await ctx.newPage()
  await login(a, login_, password)
  return { ctx, page: a }
}

test('приход: несвязанный код Умага — окно, «Завести и добавить», все коды у товара; повторный скан — тот же товар', async ({ page }) => {
  await login(page)
  const catName = `Фильтры Умаг ${RUN}`
  await category(page.request, catName)
  const name = `Фильтр Умаг ${RUN}`
  const main = `51${RUN}01`
  const extra1 = `52${RUN}01`
  const extra2 = `53${RUN}01`
  await putUmag(page.request, [
    {
      name,
      barcode: main,
      article: `UF-${RUN}`,
      extra_code: `${extra1}; ${extra2}`,
      cost_tyiyn: 30_000,
      price_tyiyn: 50_000,
      unit: 'шт',
      supplier: `Поставщик Умаг ${RUN}`,
      category: catName,
    },
  ])

  await page.goto('/receipts/new')
  const picker = page.locator('#receipt-picker')
  // Скан доп. кода: среди товаров его нет, в Умаге есть — окно строки Умага.
  await picker.fill(extra1)
  await picker.press('Enter')
  const modal = page.getByTestId('umag-modal')
  await expect(modal).toBeVisible()
  await expect(modal).toContainText(name)
  await expect(modal).toContainText(money('500 с'))
  await expect(modal).toContainText(money('300 с'))
  await expect(modal).toContainText(`Поставщик Умаг ${RUN}`)
  await expect(modal).toContainText(catName)
  await expect(modal.getByTestId('umag-create')).toHaveText('Завести и добавить')
  await shot1366(page, 'prihod-1366-umag-modal')
  await modal.getByTestId('umag-create').click()

  // Полная карточка из Умага; после сохранения — строка прихода с закупкой из Умага.
  const card = page.getByRole('dialog', { name: 'Новый товар' })
  await expect(card.getByText('Данные взяты из старой программы')).toBeVisible()
  await expect(card.getByTestId('umag-codes')).toContainText(extra2)
  await expect(card.locator('#product-name')).toHaveValue(name)
  await expect(card.locator('#product-price')).toHaveValue('500')
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/prihod-1366-umag-card.png` })
  await card.getByRole('button', { name: 'Сохранить' }).click()
  await expect(card).toBeHidden()
  await expect(page.getByText(`1. ${name}`)).toBeVisible()
  await expect(page.locator('#line-1-price')).toHaveValue('300')

  // Товар связан со строкой, все её коды — коды товара.
  const p = await apiGet<Product>(page.request, `/products/by-barcode/${main}`)
  expect(p.name).toBe(name)
  expect(p.barcodes).toEqual(expect.arrayContaining([main, extra1, extra2, `UF-${RUN}`]))
  expect((await umagRow(page.request, extra2)).product_id).toBe(p.id)

  // Повторный скан другим кодом Умага — тот же товар, без окна.
  await picker.fill(main)
  await picker.press('Enter')
  await expect(page.locator('#line-1-count')).toHaveValue('2')
  await picker.fill(extra2)
  await picker.press('Enter')
  await expect(page.locator('#line-1-count')).toHaveValue('3')
  await expect(page.getByTestId('umag-modal')).toHaveCount(0)
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/prihod-1366-umag-line.png` })
})

test('приход: «Это уже есть — выбрать товар» связывает строку Умага со своим товаром', async ({ page }) => {
  await login(page)
  const cat = await category(page.request, `Свечи Умаг ${RUN}`)
  const own = `Свеча зажигания своя ${RUN}`
  const p = await product(page.request, cat, own, [`63${RUN}04`], 30_000)
  const umagCode = `64${RUN}04`
  await putUmag(page.request, [{ name: `NGK BKR6E ${RUN}`, barcode: umagCode, cost_tyiyn: 15_000, price_tyiyn: 30_000, unit: 'шт' }])

  await page.goto('/receipts/new')
  const picker = page.locator('#receipt-picker')
  await picker.fill(umagCode)
  await picker.press('Enter')
  await page.getByTestId('umag-modal').getByTestId('umag-pick-existing').click()
  const pick = page.getByRole('dialog', { name: 'Какой это товар?' })
  await pick.getByLabel('Найти свой товар').fill(own)
  await pick.getByRole('listitem').filter({ hasText: own }).getByRole('button', { name: 'Связать' }).click()
  await expect(pick).toBeHidden()
  await expect(page.getByText(`1. ${own}`)).toBeVisible()
  expect((await apiGet<Product>(page.request, `/products/by-barcode/${umagCode}`)).id).toBe(p.id)
  expect((await umagRow(page.request, umagCode)).product_id).toBe(p.id)
})

test('«Товары»: поиск по коду и тексту показывает Умаг; скан — окно и «Завести»; блок «Умаг» в карточке', async ({ page }) => {
  await login(page)
  const catName = `Антифризы Умаг ${RUN}`
  await category(page.request, catName)
  const name = `Антифриз Умаг ${RUN} G12`
  const code = `58${RUN}05`
  const extra = `65${RUN}05`
  await putUmag(page.request, [{ name, barcode: code, extra_code: extra, cost_tyiyn: 50_000, price_tyiyn: 70_000, unit: 'шт', category: catName }])

  await page.goto('/products')
  const search = page.getByLabel('Поиск', { exact: true })
  const block = page.getByTestId('umag-block')
  // Своих таких товаров нет — ниже списка строки Умага: по тексту и по коду.
  await search.fill(`Антифриз Умаг ${RUN}`)
  await expect(block).toContainText(name)
  await expect(block.getByRole('button', { name: 'Завести' })).toBeVisible()
  await search.fill(extra)
  await expect(block).toContainText(name)
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/tovary-umag-block.png`, fullPage: true })

  // Скан (Enter) — окно строки Умага; «Завести» — карточка из Умага.
  await search.press('Enter')
  const modal = page.getByTestId('umag-modal')
  await expect(modal).toBeVisible()
  await expect(modal.getByTestId('umag-create')).toHaveText('Завести')
  await modal.getByTestId('umag-create').click()
  const card = page.getByRole('dialog', { name: 'Новый товар' })
  await expect(card.locator('#product-name')).toHaveValue(name)
  await expect(card.locator('#product-price')).toHaveValue('700')
  await card.getByRole('button', { name: 'Сохранить' }).click()
  await expect(card).toBeHidden()
  // По коду Умага товар теперь в списке, строка Умага заведена — блока нет.
  const row = page.getByRole('row', { name: new RegExp(name) })
  await expect(row).toBeVisible()
  await expect(block).toHaveCount(0)

  // Карточка товара: блок «Умаг» — строка, коды, цена; «Отвязать» оставляет коды у товара.
  await row.click()
  const umagCard = page.getByTestId('umag-card')
  await expect(umagCard).toContainText(name)
  await expect(umagCard).toContainText(extra)
  await expect(umagCard).toContainText(money('700 с'))
  if (SHOTS) await umagCard.screenshot({ path: `${SHOTS}/tovary-umag-card.png` })
  page.once('dialog', (d) => void d.accept())
  await umagCard.getByRole('button', { name: 'Отвязать' }).click()
  await expect(umagCard).toContainText('Не связан')
  expect((await apiGet<Product>(page.request, `/products/by-barcode/${extra}`)).name).toBe(name)
  expect((await umagRow(page.request, code)).product_id).toBeNull()
  // «Связать со строкой Умага» — снова связать из карточки.
  await umagCard.getByRole('button', { name: 'Связать со строкой Умага' }).click()
  await umagCard.getByLabel('Строка Умага').fill(code)
  await umagCard.getByRole('listitem').filter({ hasText: name }).getByRole('button', { name: 'Связать' }).click()
  await expect(umagCard).not.toContainText('Не связан')
  await expect(umagCard).toContainText(name)
  expect((await umagRow(page.request, code)).product_id).not.toBeNull()
})

test('касса: товаров Умага не видно; код Умага связанного товара — товар в чеке, незаведённый — «Код не найден»', async ({ page }) => {
  await login(page)
  const cat = await category(page.request, `Колодки Умаг ${RUN}`)
  const linked = `Колодки свои ${RUN}`
  const p = await product(page.request, cat, linked, [`66${RUN}06`], 120_000)
  await stock(page.request, p.id, 5, 60_000)
  const free = `Колодки только в Умаге ${RUN}`
  const freeCode = `69${RUN}07`
  const extra = `67${RUN}06`
  const zeros = `68${RUN}06`
  await putUmag(page.request, [
    { name: `Колодки тормозные Умаг ${RUN}`, barcode: `66${RUN}06`, extra_code: `${extra} ${zeros}`, price_tyiyn: 120_000, unit: 'шт' },
    { name: free, barcode: freeCode, price_tyiyn: 50_000, unit: 'шт' },
  ])
  const row = await umagRow(page.request, `66${RUN}06`)
  await apiPost(page.request, `/old-catalog/${row.id}/link`, { op_id: crypto.randomUUID(), product_id: p.id })

  await page.goto('/')
  const picker = page.getByPlaceholder(PICKER)
  // Поиск текстом: своих нет — и блока Умага нет.
  await picker.fill(free)
  await expect(page.getByText(`Ничего не нашли по «${free}»`)).toBeVisible()
  await expect(page.getByTestId('umag-block')).toHaveCount(0)
  await expect(page.locator('[data-old-hit]')).toHaveCount(0)
  // Незаведённый код Умага — прежнее «Код не найден», окна Умага нет.
  await picker.fill(freeCode)
  await picker.press('Enter')
  const unknown = page.getByRole('dialog', { name: `Код ${freeCode} не найден` })
  await expect(unknown).toBeVisible()
  await expect(page.getByTestId('umag-modal')).toHaveCount(0)
  await unknown.getByRole('button', { name: 'Отмена' }).click()
  // Код Умага с потерянными Excel нулями впереди и доп. код — связанный товар в чеке.
  await picker.fill(`00${zeros}`)
  await picker.press('Enter')
  const cart = page.getByTestId('cart')
  await expect(cart).toContainText(linked)
  await picker.fill(extra)
  await picker.press('Enter')
  await expect(page.getByRole('textbox', { name: 'Кол-во' })).toHaveValue('2')
  await expect(page.getByTestId('umag-modal')).toHaveCount(0)
})

test.describe('режим кассы 1366×768', () => {
  test.use({ viewport: { width: 1366, height: 768 }, hasTouch: true })

  test('блока и окна Умага нет; скан кода Умага связанного товара добавляет товар', async ({ page }) => {
    await login(page)
    const cat = await category(page.request, `Сенсор Умаг ${RUN}`)
    const linked = `Щётка сенсор ${RUN}`
    const p = await product(page.request, cat, linked, [`77${RUN}14`], 40_000)
    await stock(page.request, p.id, 5, 20_000)
    const free = `Щётка только в Умаге ${RUN}`
    const freeCode = `78${RUN}15`
    const umagCode = `79${RUN}14`
    await putUmag(page.request, [
      { name: `Щётка Умаг ${RUN}`, barcode: `77${RUN}14`, extra_code: umagCode, price_tyiyn: 40_000, unit: 'шт' },
      { name: free, barcode: freeCode, price_tyiyn: 30_000, unit: 'шт' },
    ])
    const row = await umagRow(page.request, umagCode)
    await apiPost(page.request, `/old-catalog/${row.id}/link`, { op_id: crypto.randomUUID(), product_id: p.id })

    await page.goto('/')
    await page.getByRole('button', { name: 'Режим кассы', exact: true }).tap()
    await expect(page.getByTestId('cashier-mode-bar')).toBeVisible()
    const picker = page.getByPlaceholder(PICKER)
    await picker.fill(free)
    await expect(page.getByText(`Ничего не нашли по «${free}»`)).toBeVisible()
    await expect(page.getByTestId('umag-block')).toHaveCount(0)
    // Сканер: цифры и Enter.
    await picker.fill(umagCode)
    await picker.press('Enter')
    await expect(page.getByTestId('cart')).toContainText(linked)
    if (SHOTS) await page.screenshot({ path: `${SHOTS}/kassa-1366-umag-linked.png` })
    await picker.fill(freeCode)
    await picker.press('Enter')
    await expect(page.getByRole('dialog', { name: `Код ${freeCode} не найден` })).toBeVisible()
    await expect(page.getByTestId('umag-modal')).toHaveCount(0)
    if (SHOTS) await page.screenshot({ path: `${SHOTS}/kassa-1366-umag-not-found.png` })
    await page.getByRole('dialog', { name: `Код ${freeCode} не найден` }).getByRole('button', { name: 'Отмена' }).tap()
    await page.getByRole('button', { name: 'Выйти из режима кассы' }).tap()
    await expect(page.getByTestId('cashier-mode-bar')).toHaveCount(0)
  })
})

test('«Связать с Умагом»: сводка, автосвязывание, новая выгрузка, похожие пары, «Не то», конфликт кода, «Отвязать»', async ({ page }) => {
  await login(page)
  const cat = await category(page.request, `Связь Умаг ${RUN}`)
  const byCode = await product(page.request, cat, `Масло по коду ${RUN}`, [`70${RUN}08`], 250_000)
  const byName = await product(page.request, cat, `Тосол «Ёлка» ${RUN}`, [], 40_000)
  const pairName = `Щётка стеклоочистителя каркасная ${RUN}`
  const pair = await product(page.request, cat, pairName, [], 60_000)
  const rejectName = `Ароматизатор ёлочка ваниль ${RUN}`
  await product(page.request, cat, rejectName, [], 15_000)
  const codeOwner = await product(page.request, cat, `Чужой код ${RUN}`, [`73${RUN}11`], 10_000)
  const target = await product(page.request, cat, `Свечи комплект ${RUN}`, [], 90_000)
  const a = { name: `MOBIL SUPER ${RUN}`, barcode: `71${RUN}08`, extra_code: `70${RUN}08`, price_tyiyn: 260_000, unit: 'шт' }
  const b = { name: `ТОСОЛ "Елка" ${RUN}`, barcode: `72${RUN}09`, price_tyiyn: 45_000, unit: 'шт' }
  await putUmag(page.request, [a, b])

  // Сводка и «Связать автоматически»: точный код и точное название при одном кандидате.
  await page.goto('/products')
  await page.getByRole('button', { name: 'Связать с Умагом' }).click()
  await expect(page).toHaveURL(/\/products\/umag$/)
  const screen = page.getByTestId('umag-page')
  await expect(screen.getByTestId('umag-rows')).toContainText('2')
  await screen.getByTestId('umag-auto-link').click()
  await expect(screen.getByText('Связано 2: по коду 1, по названию 1.')).toBeVisible()
  await expect(screen.getByTestId('umag-linked')).toContainText('2')
  expect((await umagRow(page.request, a.barcode)).product_id).toBe(byCode.id)
  expect((await umagRow(page.request, b.barcode)).product_id).toBe(byName.id)
  expect((await apiGet<Product>(page.request, `/products/by-barcode/${a.barcode}`)).id).toBe(byCode.id)

  // Новая выгрузка сохраняет связи; новые строки — пара, «не то» и строка с чужим кодом.
  const d = { name: `Щетка стеклоочистителя каркас ${RUN}`, barcode: `74${RUN}10`, price_tyiyn: 65_000, unit: 'шт' }
  const e = { name: `Ароматизатор елочка ваниль ${RUN} новый`, barcode: `75${RUN}12`, price_tyiyn: 16_000, unit: 'шт' }
  const c = { name: `Набор свечей ${RUN}`, barcode: `76${RUN}13`, extra_code: `73${RUN}11`, price_tyiyn: 90_000, unit: 'шт' }
  await putUmag(page.request, [a, b, c, d, e])
  expect((await umagRow(page.request, a.barcode)).product_id).toBe(byCode.id)
  expect((await umagRow(page.request, b.barcode)).product_id).toBe(byName.id)
  await page.reload()

  // Похожие пары: «Не то» — пара уходит и не возвращается; «Связать» — коды строки у товара.
  const pairs = screen.getByLabel('Поиск пар')
  await pairs.fill(RUN)
  const rejectRow = screen.getByTestId('umag-pair-row').filter({ hasText: rejectName }).filter({ hasText: e.name })
  await expect(rejectRow).toBeVisible()
  await shot1366(page, 'umag-link-screen-1366')
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/umag-link-screen-full.png`, fullPage: true })
  await rejectRow.getByTestId('umag-pair-reject').click()
  await expect(rejectRow).toHaveCount(0)
  const pairRow = screen.getByTestId('umag-pair-row').filter({ hasText: pairName }).filter({ hasText: d.name })
  await expect(pairRow).toBeVisible()
  await pairRow.getByTestId('umag-pair-link').click()
  await expect(pairRow).toHaveCount(0)
  expect((await apiGet<Product>(page.request, `/products/by-barcode/${d.barcode}`)).id).toBe(pair.id)
  await page.reload()
  await screen.getByLabel('Поиск пар').fill(RUN)
  await expect(screen.getByTestId('umag-pair-row').filter({ hasText: rejectName }).filter({ hasText: e.name })).toHaveCount(0)
  expect((await umagRow(page.request, e.barcode)).product_id).toBeNull()

  // Строка с кодом другого товара: «Это уже есть» — связать с выбранным; чужой код не переносится,
  // у строки отметка конфликта.
  const rows = screen.getByTestId('umag-row')
  await screen.getByLabel('Найти строку Умага').fill(c.barcode)
  const cRow = rows.filter({ hasText: c.name })
  await cRow.getByRole('button', { name: 'Это уже есть' }).click()
  const pick = page.getByRole('dialog', { name: 'Какой это товар?' })
  await pick.getByLabel('Найти свой товар').fill(`Свечи комплект ${RUN}`)
  await pick.getByRole('listitem').filter({ hasText: `Свечи комплект ${RUN}` }).getByRole('button', { name: 'Связать' }).click()
  await expect(pick).toBeHidden()
  await expect(cRow).toContainText(`связана: Свечи комплект ${RUN}`)
  await expect(cRow).toContainText(`конфликт кода: код 73${RUN}11 — у «Чужой код ${RUN}»`)
  expect((await apiGet<Product>(page.request, `/products/by-barcode/73${RUN}11`)).id).toBe(codeOwner.id)
  expect((await apiGet<Product>(page.request, `/products/by-barcode/${c.barcode}`)).id).toBe(target.id)
  await expect(screen.getByTestId('umag-conflicts')).toContainText('1')
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/umag-link-rows-full.png`, fullPage: true })

  // Карточка товара: блок «Умаг» — связанная строка; её код, оставшийся у другого товара, — отдельно.
  await page.goto('/products')
  await page.getByLabel('Поиск', { exact: true }).fill(`Свечи комплект ${RUN}`)
  await page.getByRole('row', { name: new RegExp(`Свечи комплект ${RUN}`) }).click()
  const umagCard = page.getByTestId('umag-card')
  await expect(umagCard).toContainText(c.name)
  await expect(umagCard.getByTestId('umag-card-missing')).toContainText(`73${RUN}11`)
  await expect(umagCard.getByTestId('umag-card-missing')).not.toContainText(c.barcode)
  if (SHOTS) await umagCard.screenshot({ path: `${SHOTS}/tovary-umag-card-conflict.png` })
  await page.goto('/products/umag')

  // Повтор той же операции — тот же ответ (не «строка не связана»).
  const hitC = await umagRow(page.request, c.barcode)
  const op = crypto.randomUUID()
  const first = await apiPost(page.request, `/old-catalog/${hitC.id}/unlink`, { op_id: op })
  const again = await page.request.post(`/api/v1/old-catalog/${hitC.id}/unlink`, { data: { op_id: op }, headers: DEVICE })
  expect(again.ok(), await again.text()).toBeTruthy()
  expect(await again.json()).toEqual(first)
  // Новая отвязка уже отвязанной строки — отказ.
  const twice = await page.request.post(`/api/v1/old-catalog/${hitC.id}/unlink`, { data: { op_id: crypto.randomUUID() }, headers: DEVICE })
  expect(twice.status()).toBe(409)

  // «Отвязать» на экране: коды остаются у товара.
  await screen.getByLabel('Найти строку Умага').fill(d.barcode)
  const dRow = rows.filter({ hasText: d.name })
  await expect(dRow).toContainText(`связана: ${pairName}`)
  page.once('dialog', (dlg) => void dlg.accept())
  await dRow.getByRole('button', { name: 'Отвязать' }).click()
  await expect(dRow).not.toContainText('связана:')
  expect((await apiGet<Product>(page.request, `/products/by-barcode/${d.barcode}`)).id).toBe(pair.id)
})

test('без права «Товары и штрихкоды»: в приходе и «Товарах» — только текст, экрана связывания нет', async ({ page, browser }) => {
  await login(page)
  const name = `Ремень Умаг ${RUN}`
  const code = `80${RUN}16`
  await putUmag(page.request, [{ name, barcode: code, price_tyiyn: 35_000, cost_tyiyn: 20_000, unit: 'шт' }])
  const row = await umagRow(page.request, code)
  const cat = await category(page.request, `Ремни Умаг ${RUN}`)
  const some = await product(page.request, cat, `Ремень свой ${RUN}`, [], 35_000)

  const { ctx, page: admin } = await adminWith(page, browser, ['receipts'])
  await admin.goto('/receipts/new')
  const picker = admin.locator('#receipt-picker')
  await picker.fill(code)
  await picker.press('Enter')
  const modal = admin.getByTestId('umag-modal')
  await expect(modal.getByTestId('umag-noright')).toHaveText(NO_RIGHT)
  await expect(modal.getByTestId('umag-create')).toHaveCount(0)
  await expect(modal.getByTestId('umag-pick-existing')).toHaveCount(0)
  await modal.getByRole('button', { name: 'Закрыть' }).click()
  await expect(modal).toHaveCount(0)

  // «Товары»: строку Умага видно, завести нельзя; скан — тот же текст.
  await admin.goto('/products')
  await expect(admin.getByRole('button', { name: 'Связать с Умагом' })).toHaveCount(0)
  const search = admin.getByLabel('Поиск', { exact: true })
  await search.fill(name)
  const block = admin.getByTestId('umag-block')
  await expect(block).toContainText(name)
  await expect(block.getByRole('button', { name: 'Завести' })).toHaveCount(0)
  await search.fill(code)
  await search.press('Enter')
  await expect(admin.getByTestId('umag-noright')).toBeVisible()
  await admin.goto('/products/umag')
  await expect(admin.getByTestId('umag-page')).toHaveCount(0)

  // Сервер тоже не даёт связать.
  const denied = await admin.request.post(`/api/v1/old-catalog/${row.id}/link`, { data: { op_id: crypto.randomUUID(), product_id: some.id }, headers: DEVICE })
  expect(denied.status()).toBe(403)
  await ctx.close()
})
