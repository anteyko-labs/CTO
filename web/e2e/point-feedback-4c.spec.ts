// SPEC-30 §16–17: возврат без чека — только владелец (из «Чеков» и в режиме кассы); режим кассы
// 1366×768, чек «В сервис» — данные замены в левой колонке под строками чека, страница не
// прокручивается.
import { expect, test, type APIRequestContext, type Locator, type Page } from '@playwright/test'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e4c1' }
const PICKER = /Сканируйте/
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

async function apiGet<T>(request: APIRequestContext, path: string): Promise<T> {
  const res = await request.get(`/api/v1${path}`)
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

/** Штучный товар с остатком 10 по 450 с, цена продажи 900 с. */
async function goods(request: APIRequestContext, name: string) {
  const cat = await apiPost<{ id: string }>(request, '/categories', { name: `Возврат ${RUN} ${name}`, kind: 'other' })
  const p = await apiPost<{ id: string }>(request, '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name, sale_price_tyiyn: 90_000 })
  await apiPost(request, '/receipts', {
    op_id: crypto.randomUUID(),
    paid_from: await ownerSafe(request),
    lines: [{ product_id: p.id, qty: 10, cost_tyiyn: 450_000 }],
  })
  return p
}

async function stockOf(request: APIRequestContext, name: string): Promise<number> {
  const list = await apiGet<{ name: string; stock_qty: number }[]>(request, `/products?q=${encodeURIComponent(name)}`)
  return list.find((p) => p.name === name)?.stock_qty ?? -1
}

async function noPageScroll(page: Page): Promise<boolean> {
  return page.evaluate(() => {
    const el = document.scrollingElement ?? document.documentElement
    return el.scrollHeight <= window.innerHeight + 1 && el.scrollWidth <= window.innerWidth + 1
  })
}

async function inView(page: Page, l: Locator): Promise<boolean> {
  const box = await l.boundingBox()
  const view = page.viewportSize()
  return Boolean(box && view && box.y >= 0 && box.x >= 0 && box.y + box.height <= view.height + 1 && box.x + box.width <= view.width + 1)
}

test('возврат без чека: владелец из «Чеков» — товар на склад, деньги на карту, отметка в чеках дня; администратору нельзя', async ({ page, browser }) => {
  await login(page)
  const item = `Канистра без чека ${RUN}`
  await goods(page.request, item)
  expect(await stockOf(page.request, item)).toBe(10)

  await page.goto('/sales')
  await page.getByRole('button', { name: 'Возврат без чека' }).click()
  const dialog = page.getByRole('dialog', { name: 'Возврат без чека' })
  await dialog.getByPlaceholder(PICKER).fill(item)
  await dialog.getByRole('button', { name: new RegExp(`^${item}`) }).first().click()
  await expect(dialog.getByTestId('no-receipt-line')).toHaveCount(1)
  await dialog.getByRole('textbox', { name: 'Количество' }).fill('2')
  // Сумма по умолчанию — цена продажи; меняем на 1 500 с.
  await expect(dialog.getByTestId('no-receipt-total')).toContainText(/1\s800\sс/)
  await dialog.getByRole('textbox', { name: 'Сумма строки' }).fill('1500')
  await expect(dialog.getByTestId('no-receipt-total')).toContainText(/1\s500\sс/)
  await dialog.getByRole('button', { name: 'Карта / QR банка', exact: true }).click()
  // Причина обязательна.
  const submit = dialog.getByRole('button', { name: 'Оформить возврат' })
  await expect(submit).toBeDisabled()
  await dialog.locator('#no-receipt-reason').fill('Принёс целые без чека')
  // Без открытой смены — кассир документа.
  const cashier = dialog.locator('#no-receipt-cashier')
  if (await cashier.count()) {
    const options = await cashier.locator('option').allTextContents()
    await cashier.selectOption({ label: options.find((o) => o !== '— выберите —')! })
  }
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/no-receipt-form.png` })
  await submit.click()
  const done = page.getByTestId('no-receipt-done')
  await expect(done).toContainText('Возврат без чека №')
  await expect(done).toContainText(/1\s500\sс/)
  await done.getByRole('link', { name: 'Открыть' }).click()
  await expect(page.getByRole('heading', { name: /Возврат без чека №/ })).toBeVisible()
  await expect(page.getByText('Причина: Принёс целые без чека')).toBeVisible()
  expect(await stockOf(page.request, item)).toBe(12)
  await page.goto('/sales')
  await expect(page.getByText('возврат без чека').first()).toBeVisible()

  // Администратор: кнопки нет, сервер — 403.
  const login_ = `a4c${RUN}${Math.random().toString(36).slice(2, 6)}`
  const password = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  await apiPost(page.request, '/users', { login: login_, password, full_name: `Админ ${RUN}`, role: 'admin' })
  const ctx = await browser.newContext()
  const admin = await ctx.newPage()
  await login(admin, login_, password)
  await admin.goto('/sales')
  await expect(admin.getByRole('heading', { name: 'Чеки за день' })).toBeVisible()
  await expect(admin.getByRole('button', { name: 'Возврат без чека' })).toHaveCount(0)
  const res = await admin.request.post('/api/v1/sales/return-without-receipt', {
    headers: DEVICE,
    data: { op_id: crypto.randomUUID(), method: 'card', comment: 'x', lines: [{ kind: 'piece', product_id: crypto.randomUUID(), qty: 1, amount_tyiyn: 100 }] },
  })
  expect(res.status()).toBe(403)
  await ctx.close()
})

test.describe('режим кассы 1366×768', () => {
  test.use({ viewport: { width: 1366, height: 768 }, hasTouch: true })

  test('«В сервис»: мастер, вид замены, клиент, пробег — слева под чеком; справа оплата; без прокрутки страницы', async ({ page }) => {
    await login(page)
    const item = `Масло слева ${RUN}`
    await goods(page.request, item)
    const kind = `Замена слева ${RUN}`
    await apiPost(page.request, '/services', { name: kind, price_tyiyn: 20_000, master_fee_tyiyn: 3_000 })
    await apiPost(page.request, '/employees', { full_name: `Мастер слева ${RUN}`, is_master: true })
    const party = await apiPost<{ id: string }>(page.request, '/parties', { kind: 'person', first_name: 'Слева', last_name: `Клиент${RUN}`, phone: '0700123456' })
    await apiPost(page.request, `/parties/${party.id}/vehicles`, { plate: `01KG${RUN.slice(-3)}SLV` })

    await page.goto('/')
    await page.evaluate(() => localStorage.setItem('avtodom.cashier_mode', '1'))
    await page.reload()
    await expect(page.getByTestId('cashier-mode-bar')).toBeVisible()
    // У владельца в режиме кассы — «Возврат без чека».
    await expect(page.getByRole('button', { name: 'Возврат без чека' })).toBeVisible()

    // Отложенный чек — вторая служебная плашка сверху (первая — про хранение чеков).
    await page.getByPlaceholder(/Сканируйте штрихкод/).fill(item)
    await page.getByRole('button', { name: new RegExp(`^${item}`) }).first().tap()
    await page.getByRole('button', { name: 'Отложить', exact: true }).tap()

    await page.getByPlaceholder(/Сканируйте штрихкод/).fill(item)
    await page.getByRole('button', { name: new RegExp(`^${item}`) }).first().tap()
    await page.getByRole('button', { name: 'В сервис' }).tap()
    const kinds = page.getByRole('dialog', { name: 'Вид замены' })
    await kinds.getByPlaceholder('Найти услугу').fill(kind)
    await kinds.getByTestId('service-choices').getByRole('button', { name: new RegExp(kind) }).tap()
    await kinds.getByRole('button', { name: 'Готово' }).tap()

    const left = page.getByTestId('service-left')
    await expect(left).toBeVisible()
    for (const label of ['Мастер', 'Вид замены', 'Клиент']) await expect(left.getByText(label, { exact: false }).first()).toBeInViewport()
    // Справа — тип, итог, оплата и «Провести»; данных замены там нет.
    const top = page.getByTestId('pay-top')
    expect((await top.boundingBox())!.x).toBeGreaterThan((await left.boundingBox())!.x + (await left.boundingBox())!.width - 1)
    await expect(page.getByTestId('pay-details').locator('#master-select')).toHaveCount(0)
    await expect(left.locator('#master-select')).toBeVisible()
    // Слева под строками чека: верх блока — ниже списка.
    const cart = (await page.getByTestId('cart').boundingBox())!
    expect((await left.boundingBox())!.y).toBeGreaterThanOrEqual(cart.y + cart.height - 1)

    // Клиент с машиной — появляются пробег и кнопка «Мастер рекомендовал».
    await left.getByRole('button', { name: 'Выбрать клиента' }).tap()
    const dlg = page.getByRole('dialog', { name: 'Клиент и машина' })
    await dlg.locator('[data-client-input]').fill(`Клиент${RUN}`)
    // Найденный клиент, а не «+ Новый клиент «…»» (он на экране, пока поиск не ответил); машина встаёт
    // сама, когда загрузится карточка клиента, — «Готово» раньше закроет окно без неё.
    await dlg.getByRole('button', { name: new RegExp(`^Клиент${RUN}`) }).first().tap()
    await expect(dlg.getByLabel('Машина')).not.toHaveValue('')
    await dlg.getByRole('button', { name: 'Готово' }).tap()
    await expect(left.locator('#mileage')).toBeVisible()
    await expect(left.getByText(/Пробег сейчас/)).toBeInViewport()
    await expect(left.getByRole('button', { name: 'Мастер рекомендовал' })).toBeInViewport()
    await left.locator('#master-select').selectOption({ label: `Мастер слева ${RUN}` })

    // Цели касания — не ниже 56 точек.
    await expect(left.locator('[data-client]')).toContainText(`Клиент${RUN}`)
    for (const l of [left.locator('#master-select'), left.locator('#mileage'), left.locator('[data-declined]'), left.getByRole('button', { name: 'Изменить вид' }), left.locator('[data-client]')]) {
      expect((await l.boundingBox())!.height, await l.evaluate((e) => e.id || e.textContent)).toBeGreaterThanOrEqual(56)
    }
    expect(await noPageScroll(page)).toBe(true)
    expect(await inView(page, page.getByRole('button', { name: /Провести чек/ }))).toBe(true)
    await expect(top.getByText('Итого')).toBeInViewport()
    await expect(top.getByRole('button', { name: 'Наличные', exact: true })).toBeInViewport()
    if (SHOTS) await page.screenshot({ path: `${SHOTS}/17-service-left.png` })

    // На вынос — данные клиента снова справа, блока слева нет.
    await page.getByRole('button', { name: 'На вынос' }).tap()
    await expect(left).toHaveCount(0)
    expect(await noPageScroll(page)).toBe(true)
    await page.getByRole('button', { name: 'Очистить', exact: true }).tap()
    await page.getByRole('button', { name: 'Точно очистить?' }).tap()
    await page.getByRole('button', { name: 'Выйти из режима кассы' }).tap()
  })
})
