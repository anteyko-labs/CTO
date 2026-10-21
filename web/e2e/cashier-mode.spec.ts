// Режим кассы на сенсорном моноблоке (SPEC-29, «Режим кассы»): без меню и крупно, цифры —
// экранной клавиатурой, режим помнит устройство. Касания — как пальцем (hasTouch).
// Клавиатура сама не появляется — только по кнопке, и остаётся до «Скрыть» (ADR-101).
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

/** Поле в фокусе, клавиатура закрыта: сама не появилась, открываем кнопкой «⌨» у поля (ADR-101). */
async function openPadAt(page: Page, field: Locator) {
  await field.tap()
  await expect(page.getByTestId('keypad')).toHaveCount(0)
  await page.getByTestId('keyboard-field-button').tap()
  await expect(page.getByTestId('keypad')).toBeVisible()
}

async function hidePad(page: Page) {
  await page.getByRole('group', { name: /клавиатура/i }).getByRole('button', { name: 'Скрыть', exact: true }).tap()
  await expect(page.getByTestId('keypad')).toHaveCount(0)
  await expect(page.getByTestId('keyboard')).toHaveCount(0)
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
  await openPadAt(page, pin)
  await tapKeys(page, '24689')
  await expect(pin).toHaveValue('2468')
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/cashier-mode-1366-shift.png` })
  await dialog.getByRole('button', { name: 'Открыть', exact: true }).tap()
  await expect(bar.getByTestId('mode-shift')).toContainText(cashier)
  // Окно закрылось, а клавиатура открыта, пока её не скроют.
  await expect(bar.getByRole('button', { name: '⌨ Клавиатура' })).toHaveAttribute('aria-pressed', 'true')
  await hidePad(page)

  // Товар — как сканером, дальше только касания.
  await page.getByPlaceholder(PICKER).fill(name)
  await page.getByRole('button', { name: new RegExp(`^${name}`) }).first().tap()
  const qty = page.getByRole('textbox', { name: 'Кол-во' })
  await expect(qty).toHaveValue('1')
  await expect(qty).toHaveAttribute('inputmode', 'none')
  await page.getByRole('button', { name: 'Больше' }).tap()
  await expect(qty).toHaveValue('2')
  await page.getByRole('button', { name: 'Меньше' }).tap()
  await expect(qty).toHaveValue('1')
  // Поле в фокусе — клавиатура открывается кнопкой в полосе режима.
  await qty.tap()
  await expect(page.getByTestId('keypad')).toHaveCount(0)
  await bar.getByRole('button', { name: '⌨ Клавиатура' }).tap()
  await expect(page.getByTestId('keypad')).toBeVisible()
  await tapKeys(page, '3')
  // Первая цифра заменяет подставленную единицу.
  await expect(qty).toHaveValue('3')
  await hidePad(page)

  const received = page.locator('[data-received]')
  await expect(received).toHaveAttribute('inputmode', 'none')
  await openPadAt(page, received)
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

  // Цели касания: поля и кнопки не ниже 56 точек, оплата — не ниже 64, «Провести чек» — не ниже 72.
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
  // Сумма — на самой кнопке.
  await expect(submit).toContainText(/1\s500\sс/)
  expect((await size(submit)).height).toBeGreaterThanOrEqual(72)
  for (const k of ['1', '0', 'Стереть', 'Скрыть']) {
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
    await page.getByRole('button', { name: new RegExp(`^${name}`) }).first().tap()
    await openPadAt(page, page.locator('[data-received]'))
    await page.screenshot({ path: `${SHOTS}/cashier-mode-1920.png` })
    await hidePad(page)
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

/** Страница не прокручивается: всё помещается в окно. */
async function noPageScroll(page: Page): Promise<boolean> {
  return page.evaluate(() => {
    const el = document.scrollingElement ?? document.documentElement
    return el.scrollHeight <= window.innerHeight + 1 && el.scrollWidth <= window.innerWidth + 1
  })
}

/** Элемент целиком в окне. */
async function inView(page: Page, l: Locator): Promise<boolean> {
  const box = await l.boundingBox()
  const view = page.viewportSize()
  return Boolean(box && view && box.y >= 0 && box.x >= 0 && box.y + box.height <= view.height + 1 && box.x + box.width <= view.width + 1)
}

test('режим кассы: во весь экран без прокрутки страницы, длинный чек прокручивается внутри', async ({ page }) => {
  test.setTimeout(180_000)
  await login(page)
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Экран ${RUN}`, kind: 'other' })
  const names: string[] = []
  const ids: string[] = []
  for (let i = 1; i <= 15; i++) {
    const name = `Экран ${RUN} товар ${String(i).padStart(2, '0')}`
    const p = await apiPost<{ id: string }>(page.request, '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name, sale_price_tyiyn: 10_000 * i })
    names.push(name)
    ids.push(p.id)
  }
  await apiPost(page.request, '/receipts', {
    op_id: crypto.randomUUID(),
    paid_from: await ownerSafe(page.request),
    lines: ids.map((id) => ({ product_id: id, qty: 5, cost_tyiyn: 25_000 })),
  })
  const cashier = await apiPost<{ id: string }>(page.request, '/employees', { full_name: `Кассир экран ${RUN}`, is_cashier: true, pin: '1357' })
  await closeOpenShift(page.request)
  await apiPost(page.request, '/shifts/open', { op_id: crypto.randomUUID(), cashier_employee_id: cashier.id, cashier_pin: '1357' })

  await page.goto('/')
  await page.evaluate(() => localStorage.setItem('avtodom.cashier_mode', '1'))
  await page.reload()
  await expect(page.getByTestId('cashier-mode-bar')).toBeVisible()
  await expect(page.getByText('Чек пуст')).toBeVisible()
  const submit = page.getByRole('button', { name: 'Провести чек' })
  for (const view of [{ width: 1366, height: 768 }, { width: 1920, height: 1080 }, { width: 1280, height: 800 }]) {
    await page.setViewportSize(view)
    expect(await noPageScroll(page), `пустой чек ${view.width}×${view.height}`).toBe(true)
    expect(await inView(page, submit)).toBe(true)
    if (SHOTS) await page.screenshot({ path: `${SHOTS}/kassa-${view.width}-empty.png` })
  }

  await page.setViewportSize({ width: 1366, height: 768 })
  const picker = page.getByPlaceholder(PICKER)
  const cart = page.getByTestId('cart')
  const add = async (from: number, to: number) => {
    for (let i = from; i < to; i++) {
      await picker.fill(names[i])
      // Найденный товар, а не «+ Завести товар …» (он появляется раньше списка).
      await page.getByRole('button', { name: new RegExp(`^${names[i]}`) }).first().tap()
      await expect(cart.locator('li')).toHaveCount(i + 1)
    }
  }
  await add(0, 12)
  for (const view of [{ width: 1366, height: 768 }, { width: 1920, height: 1080 }]) {
    await page.setViewportSize(view)
    expect(await noPageScroll(page), `12 строк ${view.width}×${view.height}`).toBe(true)
    if (SHOTS) {
      await page.screenshot({ path: `${SHOTS}/kassa-${view.width}-12.png` })
      await openPadAt(page, page.locator('[data-received]'))
      expect(await noPageScroll(page)).toBe(true)
      await page.screenshot({ path: `${SHOTS}/kassa-${view.width}-12-keypad.png` })
      await hidePad(page)
    }
  }

  await page.setViewportSize({ width: 1366, height: 768 })
  await add(12, 15)
  for (const view of [{ width: 1366, height: 768 }, { width: 1920, height: 1080 }]) {
    await page.setViewportSize(view)
    // 15 строк: страница на месте, прокручивается только список чека, последняя строка видна.
    expect(await noPageScroll(page), `15 строк ${view.width}×${view.height}`).toBe(true)
    const last = cart.locator('li').last()
    await expect(last).toContainText(names[14])
    expect(await inView(page, last)).toBe(true)
    const cartBox = (await cart.boundingBox())!
    const lastBox = (await last.boundingBox())!
    expect(lastBox.y + lastBox.height).toBeLessThanOrEqual(cartBox.y + cartBox.height + 1)
    // Главные кнопки на экране и крупные.
    expect(await inView(page, submit)).toBe(true)
    await expect(submit).toContainText(/12\s000\sс/)
    expect((await size(submit)).height).toBeGreaterThanOrEqual(72)
    for (const label of ['Наличные', 'Карта / QR банка', 'QR владельцу', 'В долг', 'Смешанная']) {
      const b = page.locator('[data-pay]').getByRole('button', { name: label, exact: true })
      expect(await inView(page, b), label).toBe(true)
      expect((await size(b)).height).toBeGreaterThanOrEqual(64)
    }
    for (const l of [last.getByRole('button', { name: 'Больше' }), last.getByRole('button', { name: 'Меньше' }), last.getByRole('button', { name: 'Удалить строку' })]) {
      const s = await size(l)
      expect(s.height).toBeGreaterThanOrEqual(56)
      expect(s.width).toBeGreaterThanOrEqual(56)
    }
    for (const label of ['Отложить', 'Очистить', 'Выбрать клиента']) {
      expect((await size(page.getByRole('button', { name: label, exact: true }))).height).toBeGreaterThanOrEqual(56)
    }
  }
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/kassa-1920-15.png` })

  // Удаление строки — без вопроса, но с «Вернуть».
  await cart.locator('li').last().getByRole('button', { name: 'Удалить строку' }).tap()
  await expect(cart.locator('li')).toHaveCount(14)
  await page.getByRole('button', { name: 'Вернуть', exact: true }).tap()
  await expect(cart.locator('li')).toHaveCount(15)
  await expect(cart.locator('li').last()).toContainText(names[14])

  // Клиент выбирается в окне; в колонке — короткая строка.
  await page.getByRole('button', { name: 'Выбрать клиента' }).tap()
  const clientDialog = page.getByRole('dialog', { name: 'Клиент и машина' })
  await expect(clientDialog.getByPlaceholder('Госномер, телефон, имя или ИНН')).toBeVisible()
  await clientDialog.getByRole('button', { name: 'Готово' }).tap()
  await expect(clientDialog).toHaveCount(0)

  await page.getByRole('button', { name: 'Очистить', exact: true }).tap()
  await page.getByRole('button', { name: 'Точно очистить?' }).tap()
  await expect(page.getByText('Чек пуст')).toBeVisible()
  await page.getByRole('button', { name: 'Выйти из режима кассы' }).tap()
  await expect(page.getByRole('link', { name: 'Сводка' })).toBeVisible()
  await closeOpenShift(page.request)
})

test('клавиатура режима кассы: только по кнопке, русские буквы и латиница, открыта между полями до «Скрыть»', async ({ page }) => {
  await login(page)
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Клавиатура ${RUN}`, kind: 'other' })
  const name = `клав ${RUN} щётка`
  const p = await apiPost<{ id: string }>(page.request, '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name, sale_price_tyiyn: 20_000 })
  await apiPost(page.request, '/receipts', {
    op_id: crypto.randomUUID(),
    paid_from: await ownerSafe(page.request),
    lines: [{ product_id: p.id, qty: 5, cost_tyiyn: 50_000 }],
  })

  await page.goto('/')
  await page.evaluate(() => localStorage.setItem('avtodom.cashier_mode', '1'))
  await page.reload()
  const bar = page.getByTestId('cashier-mode-bar')
  await expect(bar).toBeVisible()
  const picker = page.getByPlaceholder(PICKER)
  const kbdButton = bar.getByRole('button', { name: '⌨ Клавиатура' })

  // Поле в фокусе — клавиатура сама не появилась, рядом с полем только кнопка «⌨».
  await picker.tap()
  await expect(page.getByTestId('keyboard')).toHaveCount(0)
  await expect(page.getByTestId('keyboard-field-button')).toBeVisible()
  await expect(kbdButton).toHaveAttribute('aria-pressed', 'false')
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/kbd-1366-closed.png` })
  await kbdButton.tap()
  const kbd = page.getByTestId('keyboard')
  await expect(kbd).toBeVisible()
  await expect(kbdButton).toHaveAttribute('aria-pressed', 'true')
  // Клавиатура не закрывает поле, страница не прокручивается, клавиши крупные.
  {
    const f = (await picker.boundingBox())!
    const k = (await kbd.boundingBox())!
    expect(f.y + f.height <= k.y || k.y + k.height <= f.y).toBe(true)
    const view = page.viewportSize()!
    expect(k.x >= 0 && k.y >= 0 && k.x + k.width <= view.width && k.y + k.height <= view.height).toBe(true)
    expect(await noPageScroll(page)).toBe(true)
    for (const key of ['й', '1', 'Пробел', 'Стереть', 'Скрыть']) {
      const s = await size(kbd.getByRole('button', { name: key, exact: true }))
      expect(s.height, key).toBeGreaterThanOrEqual(56)
      expect(s.width, key).toBeGreaterThanOrEqual(56)
    }
  }
  const tapText = async (text: string) => {
    for (const ch of text) await kbd.getByRole('button', { name: ch === ' ' ? 'Пробел' : ch, exact: true }).tap()
  }
  await tapText(`клав ${RUN}`)
  await expect(picker).toHaveValue(`клав ${RUN}`)
  await expect(page.getByRole('button', { name: new RegExp(`^${name}`) })).toBeVisible()
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/kbd-1366-ru.png` })

  // Латиница, заглавная и стереть.
  await kbd.getByRole('button', { name: 'Латиница' }).tap()
  await expect(kbd).toHaveAttribute('data-lang', 'en')
  await kbd.getByRole('button', { name: 'Заглавная буква' }).tap()
  await kbd.getByRole('button', { name: 'Q', exact: true }).tap()
  await kbd.getByRole('button', { name: 'w', exact: true }).tap()
  await expect(picker).toHaveValue(`клав ${RUN}Qw`)
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/kbd-1366-lat.png` })
  await kbd.getByRole('button', { name: 'Стереть' }).tap()
  await kbd.getByRole('button', { name: 'Стереть' }).tap()
  await expect(picker).toHaveValue(`клав ${RUN}`)
  await kbd.getByRole('button', { name: 'Русские буквы' }).tap()
  await expect(kbd).toHaveAttribute('data-lang', 'ru')

  // Enter — как с клавиатуры: товар из найденного по всему тексту.
  await kbd.getByRole('button', { name: 'Enter' }).tap()
  const cart = page.getByTestId('cart')
  await expect(cart.locator('li')).toHaveCount(1)
  await expect(cart).toContainText(name)

  // Открытая клавиатура остаётся при переходе к другому полю: у «Получено» — цифры.
  const received = page.locator('[data-received]')
  await received.tap()
  await expect(page.getByTestId('keypad')).toBeVisible()
  await expect(kbd).toHaveCount(0)
  await tapKeys(page, '500')
  await expect(received).toHaveValue('500')
  const qty = page.getByRole('textbox', { name: 'Кол-во' })
  // Обратно в поиск — снова буквы, без нажатия кнопки.
  await picker.tap()
  await expect(kbd).toBeVisible()
  await tapText('щ')
  await expect(picker).toHaveValue('щ')
  await kbd.getByRole('button', { name: 'Стереть' }).tap()

  // Клиент: окно поиска клиента, имя — буквами.
  await page.getByRole('button', { name: 'Выбрать клиента' }).tap()
  const clientDialog = page.getByRole('dialog', { name: 'Клиент и машина' })
  const clientSearch = clientDialog.getByPlaceholder('Госномер, телефон, имя или ИНН')
  await clientSearch.tap()
  await expect(kbd).toBeVisible()
  await tapText('ив')
  await expect(clientSearch).toHaveValue('ив')
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/kbd-1366-client.png` })
  await clientDialog.getByRole('button', { name: 'Готово' }).tap()

  // «Скрыть» прячет её, и дальше она сама не появляется.
  await picker.tap()
  await kbd.getByRole('button', { name: 'Скрыть', exact: true }).tap()
  await expect(kbd).toHaveCount(0)
  await expect(kbdButton).toHaveAttribute('aria-pressed', 'false')
  await qty.tap()
  await expect(page.getByTestId('keypad')).toHaveCount(0)
  await picker.tap()
  await expect(page.getByTestId('keyboard')).toHaveCount(0)

  // 1920×1080 и тёмная тема: клавиатура по кнопке у поля, страница на месте.
  await page.setViewportSize({ width: 1920, height: 1080 })
  await page.getByTestId('keyboard-field-button').tap()
  await expect(kbd).toBeVisible()
  expect(await noPageScroll(page)).toBe(true)
  if (SHOTS) {
    await page.waitForTimeout(300)
    await page.screenshot({ path: `${SHOTS}/kbd-1920.png` })
    await page.setViewportSize({ width: 1366, height: 768 })
    await page.evaluate(() => document.documentElement.setAttribute('data-theme', 'dark'))
    // Кнопки меняют фон плавно — снимок после перехода.
    await page.waitForTimeout(300)
    await page.screenshot({ path: `${SHOTS}/kbd-1366-dark.png` })
    await page.evaluate(() => document.documentElement.removeAttribute('data-theme'))
  }
  await kbd.getByRole('button', { name: 'Скрыть', exact: true }).tap()

  await page.getByRole('button', { name: 'Очистить', exact: true }).tap()
  await page.getByRole('button', { name: 'Точно очистить?' }).tap()
  await expect(page.getByText('Чек пуст')).toBeVisible()
  await page.getByRole('button', { name: 'Выйти из режима кассы' }).tap()
  await expect(page.getByRole('link', { name: 'Сводка' })).toBeVisible()
})
