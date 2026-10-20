// Режим кассы на сенсорном моноблоке (SPEC-29, «Режим кассы»): без меню и крупно, цифры —
// экранной клавиатурой, режим помнит устройство. Касания — как пальцем (hasTouch).
import { expect, test, type APIRequestContext, type Locator, type Page } from '@playwright/test'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2fa' }
const RUN = Date.now().toString().slice(-8)
const PICKER = /Сканируйте штрихкод/
// Снимки экрана для проверки раскладки: только если задан каталог.
const SHOTS = process.env.E2E_SHOTS_DIR

test.use({ viewport: { width: 1366, height: 768 }, hasTouch: true })

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
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

async function closeOpenShift(request: APIRequestContext) {
  const cur = await apiGet<{ id: string; expected_tyiyn: number } | null>(request, '/shifts/current')
  if (!cur) return
  await apiPost(request, `/shifts/${cur.id}/close`, { op_id: crypto.randomUUID(), counted_tyiyn: Math.max(cur.expected_tyiyn, 0), comment: 'сквозной тест' })
  await apiPost(request, `/shifts/${cur.id}/handover`, { op_id: crypto.randomUUID(), amount_tyiyn: 0 })
}

/** Высота и ширина на экране, точки. */
async function size(l: Locator): Promise<{ width: number; height: number }> {
  const box = await l.boundingBox()
  expect(box, 'элемент не на экране').toBeTruthy()
  return { width: box!.width, height: box!.height }
}

async function tapKeys(page: Page, keys: string) {
  const pad = page.getByTestId('keypad')
  for (const k of keys) await pad.getByRole('button', { name: k === ',' ? 'Запятая' : k, exact: true }).tap()
}

test('режим кассы: крупно, без меню, продажа экранной клавиатурой, помнится после перезагрузки', async ({ page }) => {
  await login(page)
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Сенсор ${RUN}`, kind: 'other' })
  const name = `Ароматизатор сенсор ${RUN}`
  const p = await apiPost<{ id: string }>(page.request, '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name, sale_price_tyiyn: 50_000 })
  await apiPost(page.request, '/receipts', {
    op_id: crypto.randomUUID(),
    paid_from: await ownerSafe(page.request),
    lines: [{ product_id: p.id, qty: 10, cost_tyiyn: 300_000 }],
  })
  const cashier = `Кассир сенсор ${RUN}`
  await apiPost(page.request, '/employees', { full_name: cashier, is_cashier: true, pin: '2468' })
  await closeOpenShift(page.request)

  // Включаем режим кнопкой на экране кассы: меню и шапка пропадают, сверху — узкая полоса.
  await page.goto('/')
  await page.getByRole('button', { name: 'Режим кассы', exact: true }).tap()
  const bar = page.getByTestId('cashier-mode-bar')
  await expect(bar).toBeVisible()
  await expect(page.getByRole('link', { name: 'Сводка' })).toHaveCount(0)
  await expect(bar.getByTestId('mode-online')).toContainText('В сети')
  const exit = bar.getByRole('button', { name: 'Выйти из режима кассы' })
  await expect(exit).toBeVisible()

  // Смена открывается прямо из режима: PIN — экранной клавиатурой.
  await bar.getByRole('button', { name: 'Открыть смену' }).tap()
  const dialog = page.getByRole('dialog', { name: 'Открыть смену' })
  await dialog.locator('#shift-cashier').selectOption({ label: cashier })
  const pin = dialog.locator('#shift-pin')
  await expect(pin).toHaveAttribute('inputmode', 'none')
  await pin.tap()
  await expect(page.getByTestId('keypad')).toBeVisible()
  await tapKeys(page, '24689')
  await expect(pin).toHaveValue('2468')
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/cashier-mode-1366-shift.png` })
  await dialog.getByRole('button', { name: 'Открыть', exact: true }).tap()
  await expect(bar.getByTestId('mode-shift')).toContainText(cashier)

  // Товар — как сканером, дальше только касания.
  await page.getByPlaceholder(PICKER).fill(name)
  await page.getByRole('button', { name: new RegExp(name) }).first().tap()
  const qty = page.getByRole('textbox', { name: 'Кол-во' })
  await expect(qty).toHaveValue('1')
  await expect(qty).toHaveAttribute('inputmode', 'none')
  await page.getByRole('button', { name: 'Больше' }).tap()
  await expect(qty).toHaveValue('2')
  await page.getByRole('button', { name: 'Меньше' }).tap()
  await expect(qty).toHaveValue('1')
  await qty.tap()
  await tapKeys(page, '3')
  // Первая цифра заменяет подставленную единицу.
  await expect(qty).toHaveValue('3')
  await page.getByTestId('keypad').getByRole('button', { name: 'Готово' }).tap()
  await expect(page.getByTestId('keypad')).toHaveCount(0)

  const received = page.locator('[data-received]')
  await expect(received).toHaveAttribute('inputmode', 'none')
  await received.tap()
  await tapKeys(page, '2000')
  await expect(received).toHaveValue('2000')
  await expect(page.getByText('Сдача').locator('..')).toContainText(/500\sс/)
  // Клавиатура — рядом с полем «Получено», не поверх него, и целиком на экране.
  {
    const f = await received.boundingBox()
    const k = await page.getByTestId('keypad').boundingBox()
    expect(f && k).toBeTruthy()
    const overlap = f!.x < k!.x + k!.width && k!.x < f!.x + f!.width && f!.y < k!.y + k!.height && k!.y < f!.y + f!.height
    expect(overlap).toBe(false)
    const view = page.viewportSize()!
    expect(k!.x >= 0 && k!.y >= 0 && k!.x + k!.width <= view.width && k!.y + k!.height <= view.height).toBe(true)
  }

  // Цели касания: поля и кнопки не ниже 56 точек, оплата и «Провести чек» — не ниже 64.
  for (const l of [page.getByRole('button', { name: 'Больше' }), page.getByRole('button', { name: 'Меньше' }), page.getByRole('button', { name: 'Удалить строку' })]) {
    const s = await size(l)
    expect(s.height).toBeGreaterThanOrEqual(56)
    expect(s.width).toBeGreaterThanOrEqual(56)
  }
  expect((await size(qty)).height).toBeGreaterThanOrEqual(56)
  expect((await size(received)).height).toBeGreaterThanOrEqual(56)
  expect((await size(page.getByPlaceholder(PICKER))).height).toBeGreaterThanOrEqual(56)
  for (const label of ['Наличные', 'Карта / QR банка', 'В долг']) {
    expect((await size(page.locator('[data-pay]').getByRole('button', { name: label, exact: true }))).height).toBeGreaterThanOrEqual(64)
  }
  const submit = page.getByRole('button', { name: 'Провести чек' })
  expect((await size(submit)).height).toBeGreaterThanOrEqual(64)
  for (const k of ['1', '0', 'Стереть', 'Готово']) {
    expect((await size(page.getByTestId('keypad').getByRole('button', { name: k, exact: true }))).height).toBeGreaterThanOrEqual(56)
  }
  // Без прокрутки вбок.
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true)
  if (SHOTS) {
    await page.screenshot({ path: `${SHOTS}/cashier-mode-1366-keypad.png` })
    await page.evaluate(() => window.scrollTo(0, 0))
    await page.screenshot({ path: `${SHOTS}/cashier-mode-1366.png`, fullPage: true })
  }

  await submit.tap()
  await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()
  await expect(page.getByText('Чек пуст')).toBeVisible()

  // Режим помнит устройство: после перезагрузки касса сразу в нём.
  await page.reload()
  await expect(page.getByTestId('cashier-mode-bar')).toBeVisible()
  await expect(page.getByRole('link', { name: 'Сводка' })).toHaveCount(0)

  await page.setViewportSize({ width: 1920, height: 1080 })
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true)
  if (SHOTS) {
    await page.getByPlaceholder(PICKER).fill(name)
    await page.getByRole('button', { name: new RegExp(name) }).first().tap()
    await page.locator('[data-received]').tap()
    await page.screenshot({ path: `${SHOTS}/cashier-mode-1920.png` })
    await page.getByTestId('keypad').getByRole('button', { name: 'Готово' }).tap()
    await page.getByRole('button', { name: 'В сервис' }).tap()
    await page.screenshot({ path: `${SHOTS}/cashier-mode-1920-service.png`, fullPage: true })
    await page.getByRole('button', { name: 'На вынос' }).tap()
    await page.getByRole('button', { name: 'Удалить строку' }).tap()
  }

  // Выход — меню на месте, и после перезагрузки режим не включается.
  await page.getByRole('button', { name: 'Выйти из режима кассы' }).tap()
  await expect(page.getByTestId('cashier-mode-bar')).toHaveCount(0)
  await expect(page.getByRole('link', { name: 'Сводка' })).toBeVisible()
  await page.reload()
  await expect(page.getByRole('link', { name: 'Сводка' })).toBeVisible()
  await expect(page.getByTestId('cashier-mode-bar')).toHaveCount(0)

  await closeOpenShift(page.request)
})
