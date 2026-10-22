// Доработки SPEC-30 после проверки вживую: машина клиента встаёт сама, вид замены — отметками
// с «Готово», удаление отложенного чека — с подтверждением, права без «Прихода» и «Товаров»,
// режим кассы 1366×768 — оплата закреплена, «Не хватает» одной строкой.
import { expect, test, type APIRequestContext, type Browser, type Page } from '@playwright/test'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e4b1' }
const PICKER = /Сканируйте штрихкод/
const RUN = Date.now().toString().slice(-8)
// Снимки экрана для проверки раскладки: только если задан каталог.
const SHOTS = process.env.E2E_SHOTS_DIR

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

/** Штучный товар с остатком. */
async function goods(request: APIRequestContext, name: string, price = 90_000) {
  const cat = await apiPost<{ id: string }>(request, '/categories', { name: `Разное ${RUN} ${name}`, kind: 'other' })
  const p = await apiPost<{ id: string }>(request, '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name, sale_price_tyiyn: price })
  await apiPost(request, '/receipts', {
    op_id: crypto.randomUUID(),
    paid_from: await ownerSafe(request),
    lines: [{ product_id: p.id, qty: 20, cost_tyiyn: 20 * (price / 2) }],
  })
  return p
}

async function addToCart(page: Page, name: string) {
  await page.getByPlaceholder(PICKER).fill(name)
  await page.getByRole('button', { name: new RegExp(`^${name}`) }).first().click()
  await expect(page.getByTestId('cart')).toContainText(name)
}

/** Администратор с выбранными правами; страница уже вошла. */
async function adminWith(page: Page, browser: Browser, permissions: string[]) {
  const login_ = `a4b${RUN}${Math.random().toString(36).slice(2, 6)}`
  const password = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  const user = await apiPost<{ id: string }>(page.request, '/users', { login: login_, password, full_name: `Админ ${RUN}`, role: 'admin' })
  const res = await page.request.put(`/api/v1/users/${user.id}/permissions`, { data: { op_id: crypto.randomUUID(), permissions }, headers: DEVICE })
  expect(res.ok(), await res.text()).toBeTruthy()
  const ctx = await browser.newContext()
  const a = await ctx.newPage()
  await login(a, login_, password)
  return { ctx, page: a }
}

test('клиент по имени: единственная машина встаёт сама; удаление отложенного — с подтверждением', async ({ page }) => {
  await login(page)
  const party = await apiPost<{ id: string }>(page.request, '/parties', { kind: 'person', first_name: 'Машинов', last_name: `Одна${RUN}` })
  const car = await apiPost<{ id: string }>(page.request, `/parties/${party.id}/vehicles`, { plate: `01KG${RUN.slice(-3)}AAA` })
  const item = `Щётка ${RUN}`
  await goods(page.request, item)

  await page.goto('/')
  await addToCart(page, item)
  // Ищем по фамилии, не по госномеру — машина всё равно встаёт.
  await page.locator('[data-client-input]').fill(`Одна${RUN}`)
  await page.getByRole("button", { name: new RegExp(`^Одна${RUN}`) }).click()
  await expect(page.locator('[data-client] select').last()).toHaveValue(car.id)

  // Отложить, затем удалить отложенный: сначала «Точно удалить?», «Нет» — чек остаётся.
  await page.getByRole('button', { name: 'Отложить', exact: true }).click()
  const del = page.getByRole('button', { name: 'Удалить отложенный чек' })
  await expect(del).toHaveCount(1)
  await del.click()
  await expect(page.getByRole('button', { name: 'Точно удалить?' })).toBeVisible()
  await page.getByRole('button', { name: 'Нет', exact: true }).click()
  await expect(del).toHaveCount(1)
  await del.click()
  await page.getByRole('button', { name: 'Точно удалить?' }).click()
  const chip = page.getByText('Отложенные:')
  await expect(chip).toHaveCount(0)
})

test.describe('режим кассы 1366×768', () => {
  test.use({ viewport: { width: 1366, height: 768 }, hasTouch: true })

  test('вид замены отметками, оплата закреплена, «Не хватает» одной строкой', async ({ page }) => {
    await login(page)
    const item = `Масло-тест ${RUN}`
    await goods(page.request, item, 250_000)
    const kinds = [`Замена А ${RUN}`, `Замена Б ${RUN}`]
    for (const name of kinds) await apiPost(page.request, '/services', { name, price_tyiyn: 20_000, master_fee_tyiyn: 3_000 })

    await page.goto('/')
    await page.getByRole('button', { name: 'Режим кассы', exact: true }).click()
    await expect(page.getByTestId('cashier-mode-bar')).toBeVisible()
    await addToCart(page, item)

    // «В сервис» — окно «Вид замены»: помещается в экран, отмечаем два вида, закрывает «Готово».
    await page.getByRole('button', { name: 'В сервис' }).click()
    const dialog = page.getByRole('dialog', { name: 'Вид замены' })
    await expect(dialog).toBeVisible()
    const box = await dialog.boundingBox()
    expect(box!.y + box!.height).toBeLessThanOrEqual(768)
    await dialog.getByPlaceholder('Найти услугу').fill(RUN)
    for (const name of kinds) {
      const tile = dialog.getByTestId('service-choices').getByRole('button', { name: new RegExp(name) })
      await tile.click()
      await expect(tile).toHaveAttribute('aria-pressed', 'true')
    }
    await expect(dialog).toBeVisible()
    if (SHOTS) await page.screenshot({ path: `${SHOTS}/4b-vid-zameny.png` })
    await dialog.getByRole('button', { name: 'Готово' }).click()
    await expect(dialog).toHaveCount(0)
    for (const name of kinds) await expect(page.getByTestId('cart')).toContainText(name)
    await expect(page.getByTestId('service-kinds')).toContainText(/Мастеру начислится 60\sс/)

    // Снять отметку — вид уходит из чека.
    await page.getByRole('button', { name: 'Изменить вид' }).click()
    await dialog.getByPlaceholder('Найти услугу').fill(kinds[1])
    await dialog.getByTestId('service-choices').getByRole('button', { name: new RegExp(kinds[1]) }).click()
    await dialog.getByRole('button', { name: 'Готово' }).click()
    await expect(page.getByTestId('cart')).not.toContainText(kinds[1])

    // Чего не хватает — одной строкой.
    const short = page.getByTestId('short-missing')
    await expect(short).toContainText('Не хватает:')
    await expect(short).toContainText('мастер')
    expect((await short.boundingBox())!.height).toBeLessThan(60)
    if (SHOTS) await page.screenshot({ path: `${SHOTS}/4b-service.png` })
    // Касание ведёт к первому недостающему: кассир (смены нет) или мастер.
    await short.click()
    await expect.poll(() => page.evaluate(() => document.activeElement?.id)).toMatch(/^(cashier|master)-select$/)

    // Итог и оплата закреплены: прокрутили данные клиента вниз — «Итого» и способы оплаты на месте.
    const top = page.getByTestId('pay-top')
    await page.getByTestId('pay-details').evaluate((el) => el.scrollTo(0, el.scrollHeight))
    await expect(top.getByText('Итого')).toBeInViewport()
    await expect(top.getByRole('button', { name: 'Наличные', exact: true })).toBeInViewport()
    if (SHOTS) await page.screenshot({ path: `${SHOTS}/4b-service-scrolled.png` })

    // Смешанная оплата: все поля видны.
    await top.getByRole('button', { name: 'Смешанная' }).click()
    for (const label of ['Наличные', 'Карта / QR банка', 'QR владельцу', 'По счёту', 'В долг']) {
      await expect(page.getByTestId('mixed-touch').getByRole('textbox', { name: label, exact: true })).toBeInViewport({ ratio: 1 })
    }
    if (SHOTS) await page.screenshot({ path: `${SHOTS}/4b-mixed.png` })
    // Страница не прокручивается.
    const scroll = await page.evaluate(() => document.documentElement.scrollHeight - window.innerHeight)
    expect(scroll).toBeLessThanOrEqual(1)
    await top.getByRole('button', { name: 'Наличные', exact: true }).click()
    if (SHOTS) await page.screenshot({ path: `${SHOTS}/4b-cash.png` })

    // Клиент в сервисе — обязателен: подпись в окне.
    await page.locator('[data-client]').click()
    await expect(page.getByRole('dialog', { name: 'Клиент и машина' })).toContainText('Для чека «В сервис» обязателен')
    if (SHOTS) await page.screenshot({ path: `${SHOTS}/4b-client-dialog.png` })
    await page.getByRole('dialog', { name: 'Клиент и машина' }).getByRole('button', { name: 'Готово' }).click()

    // Кнопка «⌨» у поиска переезжает вместе с полем, когда над ним появляются «Отложенные».
    await page.getByRole('button', { name: 'Отложить', exact: true }).click()
    const picker = page.getByPlaceholder(PICKER)
    await picker.click()
    const kb = page.getByTestId('keyboard-field-button')
    await expect(kb).toBeVisible()
    const p = (await picker.boundingBox())!
    const b = (await kb.boundingBox())!
    expect(b.y).toBeGreaterThanOrEqual(p.y - 2)
    expect(b.y + b.height).toBeLessThanOrEqual(p.y + p.height + 2)
    if (SHOTS) await page.screenshot({ path: `${SHOTS}/4b-deferred.png` })
    // Полная клавиатура: кыргызские буквы и что набрано.
    await kb.click()
    const keyboard = page.getByTestId('keyboard')
    await expect(keyboard).toBeVisible()
    await keyboard.getByRole('button', { name: 'ө', exact: true }).click()
    await keyboard.getByRole('button', { name: 'ң', exact: true }).click()
    await expect(picker).toHaveValue('өң')
    await expect(page.getByTestId('keyboard-value')).toHaveText('өң')
    if (SHOTS) await page.screenshot({ path: `${SHOTS}/4b-keyboard.png` })
    await keyboard.getByRole('button', { name: 'Скрыть', exact: true }).click()
    await page.getByRole('button', { name: 'Выйти из режима кассы' }).click()
  })
})

test('права: без «Прихода» и «Оприходования» — только «Нет права»; карточка товара без «Товаров» — только чтение', async ({ page, browser }) => {
  await login(page)
  // Без ключа распознавания владелец видит, как включить «Заполнить по фото».
  await page.goto('/receipts/new')
  // В CI сервер запущен с подставным ключом — тогда кнопка активна и подсказки нет.
  const drafts = (await (await page.request.get('/api/v1/receipt-drafts')).json()) as { enabled: boolean }
  if (!drafts.enabled) await expect(page.getByTestId('photo-needs-key')).toContainText('нужен ключ распознавания')

  const { ctx, page: a } = await adminWith(page, browser, ['clients'])
  await a.goto('/receipts/new')
  await expect(a.getByTestId('no-right')).toContainText('Нет права')
  await expect(a.locator('#receipt-picker')).toHaveCount(0)
  const res = await a.request.post('/api/v1/suppliers', { data: { name: `Поставщик ${RUN}`, phone: '', comment: '' }, headers: DEVICE })
  expect(res.status()).toBe(403)
  const party = await a.request.post('/api/v1/parties', { data: { role: 'supplier', kind: 'company', name: `Поставщик ${RUN}` }, headers: DEVICE })
  expect(party.status()).toBe(403)
  await a.goto('/suppliers')
  await expect(a.getByRole('button', { name: 'Добавить' })).toHaveCount(0)

  // Фильтр: кросс-номера одним полем, без права — поля недоступны.
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Фильтры ${RUN}`, kind: 'filter' })
  const name = `Фильтр карточка ${RUN}`
  await apiPost(page.request, '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name, sale_price_tyiyn: 60_000 })
  await a.goto('/products')
  await a.getByPlaceholder('Название, бренд, артикул, штрихкод').fill(name)
  await a.getByText(name).first().click()
  const card = a.getByRole('dialog', { name: 'Товар' })
  await expect(card).toBeVisible()
  await expect(card.locator('#product-name')).toBeDisabled()
  await expect(card.locator('#product-price')).toBeDisabled()
  await expect(card.getByText('Кросс-номера', { exact: true })).toHaveCount(1)
  await expect(card.getByRole('button', { name: 'Сохранить' })).toHaveCount(0)
  await ctx.close()
})
