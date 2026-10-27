// Отчёт смены, как в Умаге, и отменённые товары (SPEC-30 §7, §8): строка, удалённая крестиком,
// уходит с чеком, «Очистить» — отдельной записью; отчёт открывается из «Смены», вкладки
// показывают документы смены.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e301' }
const RUN = Date.now().toString().slice(-8)
const PICKER = /Сканируйте штрихкод/
const money = (text: string) => new RegExp(text.replace(/ /g, '\\s'))

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

/** Закрыть открытую смену без расхождения и отметить, что деньги остались в кассе. */
async function closeOpenShift(request: APIRequestContext) {
  const cur = await apiGet<{ id: string; expected_tyiyn: number } | null>(request, '/shifts/current')
  if (!cur) return
  await apiPost(request, `/shifts/${cur.id}/close`, { op_id: crypto.randomUUID(), counted_tyiyn: Math.max(cur.expected_tyiyn, 0), comment: 'сквозной тест' })
  await apiPost(request, `/shifts/${cur.id}/handover`, { op_id: crypto.randomUUID(), amount_tyiyn: 0 })
}

async function scan(page: Page, code: string) {
  const picker = page.getByPlaceholder(PICKER)
  await picker.fill(code)
  await picker.press('Enter')
}

test('отчёт смены: движение денег, вкладки и отменённые товары', async ({ page }) => {
  await login(page)
  const cashier = `Кассир отчёта ${RUN}`
  const filter = `Фильтр отчёта ${RUN}`
  const oil = `Присадка отчёта ${RUN}`
  const codeA = `27${RUN}101`
  const codeB = `27${RUN}102`
  const emp = await apiPost<{ id: string }>(page.request, '/employees', { full_name: cashier, is_cashier: true, pin: '1234' })
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Отчёт ${RUN}`, kind: 'filter' })
  const a = await apiPost<{ id: string }>(page.request, '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name: filter, sale_price_tyiyn: 50_000, barcodes: [codeA] })
  const b = await apiPost<{ id: string }>(page.request, '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name: oil, sale_price_tyiyn: 30_000, barcodes: [codeB] })
  await apiPost(page.request, '/receipts', {
    op_id: crypto.randomUUID(),
    payment: 'paid',
    paid_from: await ownerSafe(page.request),
    lines: [
      { product_id: a.id, qty: 10, cost_tyiyn: 200_000 },
      { product_id: b.id, qty: 10, cost_tyiyn: 100_000 },
    ],
  })
  await closeOpenShift(page.request)
  const shift = await apiPost<{ id: string; number: number }>(page.request, '/shifts/open', {
    op_id: crypto.randomUUID(),
    cashier_employee_id: emp.id,
    cashier_pin: '1234',
  })
  await apiPost(page.request, '/cash/movements', { op_id: crypto.randomUUID(), kind: 'cash_in', amount_tyiyn: 100_000, comment: `размен ${RUN}` })

  // Касса: две строки, присадку убрали крестиком, фильтр продали.
  await page.goto('/')
  await scan(page, codeA)
  await expect(page.getByTestId('cart').getByText(filter)).toBeVisible()
  await scan(page, codeB)
  const cart = page.getByTestId('cart')
  await expect(cart.getByText(oil)).toBeVisible()
  await cart.locator('li', { hasText: oil }).getByRole('button', { name: 'Удалить строку' }).click()
  await expect(cart.getByText(oil)).toHaveCount(0)
  await page.getByRole('button', { name: 'Провести чек' }).first().click()
  await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()

  // «Очистить»: строка уходит отдельной записью через очередь устройства.
  await scan(page, codeB)
  await expect(cart.getByText(oil)).toBeVisible()
  await page.getByRole('button', { name: 'Очистить', exact: true }).click()
  await page.getByRole('button', { name: 'Точно очистить?' }).click()
  await expect(page.getByText('Чек пуст')).toBeVisible()
  await expect
    .poll(async () => (await apiGet<{ cancelled: unknown[] }>(page.request, `/shifts/${shift.id}/report`)).cancelled.length, { timeout: 40_000 })
    .toBe(2)

  // Отчёт открывается из «Смены».
  await page.goto('/shift')
  await page.locator(`a[href="/shifts/${shift.id}"]`).first().click()
  await expect(page).toHaveURL(new RegExp(`/shifts/${shift.id}$`))
  await expect(page.getByRole('heading', { name: `Смена № ${shift.number}` })).toBeVisible()
  await expect(page.getByText(`Кассир: ${cashier}`)).toBeVisible()

  // Обзор: на начало + приход − расход = должно быть; выручка 500 с, наличными.
  const flow = page.getByTestId('cash-flow')
  await expect(flow.getByTestId('flow-cash_in')).toContainText(money('1 000 с'))
  await expect(flow.getByTestId('flow-cash_sales')).toContainText(money('500 с'))
  const report = await apiGet<{ cash: { opening_tyiyn: number; income_tyiyn: number; outgo_tyiyn: number; expected_tyiyn: number } }>(page.request, `/shifts/${shift.id}/report`)
  expect(report.cash.opening_tyiyn + report.cash.income_tyiyn - report.cash.outgo_tyiyn).toBe(report.cash.expected_tyiyn)
  await expect(page.getByTestId('revenue-net')).toContainText(money('500 с'))
  await expect(page.getByTestId('method-cash')).toContainText(money('500 с'))
  // Владельцу — валовая прибыль: 500 − 200 = 300 с.
  await expect(page.getByTestId('gross')).toContainText(money('300 с'))
  await expect(page.getByTestId('hour-bars')).toBeVisible()

  // Продажи: чек смены.
  await page.getByRole('tab', { name: /Продажи/ }).click()
  await expect(page.getByRole('cell', { name: 'Наличные' }).first()).toBeVisible()
  await page.getByTestId('sales-search').fill('999999999')
  await expect(page.getByText('Ничего не нашлось')).toBeVisible()
  await page.getByTestId('sales-search').fill('')

  // Денежные операции: внесение с комментарием.
  await page.getByRole('tab', { name: /Денежные операции/ }).click()
  await expect(page.getByTestId('money-row').filter({ hasText: `размен ${RUN}` })).toContainText('Внесение')
  await expect(page.getByTestId('money-total-cash_in')).toContainText(money('1 000 с'))

  // Отменённые товары: удалённая крестиком — с номером проведённого чека, «Очистить» — без чека.
  await page.getByRole('tab', { name: /Отменённые товары/ }).click()
  const rows = page.getByTestId('cancelled-row')
  await expect(rows).toHaveCount(2)
  await expect(rows.filter({ hasText: 'Удалена из чека' })).toContainText(oil)
  await expect(rows.filter({ hasText: 'Удалена из чека' })).toContainText(/проведён № \d+/)
  await expect(rows.filter({ hasText: '«Очистить»' })).toContainText('не проведён')
  await expect(page.getByTestId('cancelled-total')).toContainText(money('600 с'))
  await expect(page).toHaveURL(/tab=cancelled/)

  await closeOpenShift(page.request)
})

// ---------- Чек смены, чистая прибыль и безнал по счетам (SPEC-36 §9, §10) ----------

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

const prints = async (page: Page) =>
  (await page.evaluate(() => (window as unknown as { __prints?: string[] }).__prints ?? [])).map((h) =>
    h.replace(/<style[\s\S]*?<\/style>/g, '').replace(/<[^>]+>/g, ' ').replace(/&nbsp;| /g, ' ').replace(/\s+/g, ' '),
  )

async function apiPut(request: APIRequestContext, path: string, data: unknown) {
  const res = await request.put(`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
}

/** Кассир, товар на складе и открытая смена с продажей наличными и картой. */
async function shiftWithSales(request: APIRequestContext, tag: string) {
  const cashier = `Кассир чека ${tag} ${RUN}`
  const emp = await apiPost<{ id: string }>(request, '/employees', { full_name: cashier, is_cashier: true, pin: '1357' })
  const cat = await apiPost<{ id: string }>(request, '/categories', { name: `Чек смены ${tag} ${RUN}`, kind: 'filter' })
  const p = await apiPost<{ id: string }>(request, '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name: `Фильтр ${tag} ${RUN}`, sale_price_tyiyn: 50_000 })
  await apiPost(request, '/receipts', {
    op_id: crypto.randomUUID(),
    payment: 'paid',
    paid_from: await ownerSafe(request),
    lines: [{ product_id: p.id, qty: 10, cost_tyiyn: 100_000 }],
  })
  await closeOpenShift(request)
  const shift = await apiPost<{ id: string; number: number }>(request, '/shifts/open', {
    op_id: crypto.randomUUID(),
    cashier_employee_id: emp.id,
    cashier_pin: '1357',
  })
  for (const method of ['cash', 'card']) {
    await apiPost(request, '/sales', {
      op_id: crypto.randomUUID(),
      sale_type: 'takeaway',
      cashier_id: emp.id,
      lines: [{ kind: 'piece', product_id: p.id, qty: 1, unit_price_tyiyn: 50_000 }],
      payments: [{ method, amount_tyiyn: 50_000 }],
    })
  }
  return { shift, cashier }
}

test('чек смены: безнал по счетам, чистая прибыль, печать после сдачи кассы и настройка', async ({ page }) => {
  await page.addInitScript(PRINT_STUB)
  await login(page)
  await apiPut(page.request, '/settings/shift', { print_on_close: true })
  const { shift } = await shiftWithSales(page.request, 'А')

  // «Смена»: безнал по счетам виден и при одном счёте — с «Упало на счёт».
  await page.goto('/shift')
  const bank = page.getByTestId('shift-bank')
  await expect(bank).toContainText('Безнал по счетам')
  await expect(bank.getByTestId('bank-account').first()).toContainText(money('500 с'))
  await expect(bank.getByTestId('bank-landed').first()).toContainText('Упало на счёт')

  // Закрыть смену: в окне тот же безнал; после «Деньги остались в кассе» чек смены печатается сам.
  await page.getByRole('button', { name: 'Закрыть смену' }).click()
  const dialog = page.getByRole('dialog', { name: 'Закрыть смену' })
  await expect(dialog.getByTestId('close-noncash')).toContainText('Упало на счёт')
  await dialog.getByRole('button', { name: 'Закрыть', exact: true }).last().click()
  await page.getByRole('button', { name: 'Деньги остались в кассе' }).click()
  await expect.poll(async () => (await prints(page)).some((t) => t.includes(`Чек смены № ${shift.number}`))).toBe(true)
  const check = (await prints(page)).find((t) => t.includes(`Чек смены № ${shift.number}`)) ?? ''
  expect(check).toContain('Безнал по счетам')
  expect(check).toContain('Выручка 1 000,00 с')
  expect(check).toContain('Валовая прибыль 800,00 с')
  expect(check).toContain('Чистая прибыль')
  expect(check).toContain('Осталось в кассе')

  // «Смена № N»: чистая прибыль владельцу и кнопка «Печать чека смены» рядом с «Печать».
  await page.goto(`/shifts/${shift.id}`)
  await expect(page.getByTestId('tile-net-profit')).toBeVisible()
  await expect(page.getByTestId('net-profit')).toContainText('без оклада')
  await expect(page.getByTestId('net-profit-total')).toContainText('Чистая прибыль за смену')
  const before = (await prints(page)).length
  await page.getByRole('button', { name: 'Печать чека смены' }).click()
  await expect.poll(async () => (await prints(page)).length).toBe(before + 1)

  // Список смен: своя кнопка у каждой смены. Настройка выключает автопечать.
  await page.goto('/shift')
  const row = page.getByRole('button', { name: `Печать чека смены № ${shift.number}` })
  await row.click()
  await expect.poll(async () => (await prints(page)).length).toBe(1)
  const toggle = page.getByTestId('print-on-close').getByRole('checkbox')
  await expect(toggle).toBeChecked()
  await toggle.uncheck()
  await expect(page.getByText('Чек смены сам не печатается')).toBeVisible()
  const next = await shiftWithSales(page.request, 'Б')
  await page.reload()
  await page.getByRole('button', { name: 'Закрыть смену' }).click()
  await page.getByRole('dialog', { name: 'Закрыть смену' }).getByRole('button', { name: 'Закрыть', exact: true }).last().click()
  await page.getByRole('button', { name: 'Деньги остались в кассе' }).click()
  await expect(page.getByText('Отмечено: деньги остались в кассе')).toBeVisible()
  await page.waitForTimeout(1_000)
  expect((await prints(page)).some((t) => t.includes(`Чек смены № ${next.shift.number}`))).toBe(false)
  await apiPut(page.request, '/settings/shift', { print_on_close: true })
})

test('чек смены у администратора — без себестоимости и прибыли', async ({ page, browser }) => {
  await login(page)
  const { shift } = await shiftWithSales(page.request, 'В')
  const login_ = `p4a${RUN}${Math.random().toString(36).slice(2, 6)}`
  const password = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  await apiPost(page.request, '/users', { login: login_, password, full_name: `Админ ${RUN}`, role: 'admin' })
  const ctx = await browser.newContext()
  await ctx.addInitScript(PRINT_STUB)
  const admin = await ctx.newPage()
  await admin.goto('/login')
  await admin.getByLabel('Логин').fill(login_)
  await admin.getByLabel('Пароль').fill(password)
  await admin.getByRole('button', { name: 'Войти' }).click()
  await expect(admin.getByRole('link', { name: 'Касса' })).toBeVisible()
  await admin.goto(`/shifts/${shift.id}`)
  await expect(admin.getByTestId('shift-bank')).toContainText('Упало на счёт')
  await expect(admin.getByTestId('tile-net-profit')).toHaveCount(0)
  await expect(admin.getByTestId('net-profit')).toHaveCount(0)
  await admin.getByRole('button', { name: 'Печать чека смены' }).click()
  await expect.poll(async () => (await prints(admin)).length).toBe(1)
  const check = (await prints(admin))[0]
  expect(check).toContain(`Чек смены № ${shift.number}`)
  expect(check).toContain('Безнал по счетам')
  expect(check).not.toContain('Себестоимость')
  expect(check).not.toMatch(/прибыль/i)
  await ctx.close()
  await closeOpenShift(page.request)
})

test.describe('режим кассы 1366×768: «Чек смены» и «Закрыть смену»', () => {
  test.use({ viewport: { width: 1366, height: 768 }, hasTouch: true })

  test('кнопки в полосе режима — от 56 точек, закрытие — то же окно', async ({ page }) => {
    await page.addInitScript(PRINT_STUB)
    await login(page)
    await apiPut(page.request, '/settings/shift', { print_on_close: true })
    const { shift } = await shiftWithSales(page.request, 'Г')
    await page.goto('/')
    await page.evaluate(() => localStorage.setItem('avtodom.cashier_mode', '1'))
    await page.reload()
    const bar = page.getByTestId('cashier-mode-bar')
    await expect(bar.getByTestId('mode-shift')).toContainText(`Смена № ${shift.number}`)
    const printBtn = bar.getByRole('button', { name: 'Чек смены', exact: true })
    const closeBtn = bar.getByRole('button', { name: 'Закрыть смену', exact: true })
    for (const b of [printBtn, closeBtn, bar.getByRole('button', { name: 'Выйти из режима кассы' })]) {
      const box = (await b.boundingBox())!
      expect(box.height).toBeGreaterThanOrEqual(56)
      expect(box.width).toBeGreaterThanOrEqual(56)
      // Полоса — одной строкой, кнопки в пределах экрана.
      expect(box.x + box.width).toBeLessThanOrEqual(1366)
    }
    const noScroll = () =>
      page.evaluate(() => {
        const el = document.scrollingElement ?? document.documentElement
        return el.scrollHeight <= window.innerHeight + 1 && el.scrollWidth <= window.innerWidth + 1
      })
    expect(await noScroll()).toBe(true)

    await printBtn.tap()
    await expect.poll(async () => (await prints(page)).some((t) => t.includes(`Чек смены № ${shift.number}`))).toBe(true)

    await closeBtn.tap()
    const dialog = page.getByRole('dialog', { name: 'Закрыть смену' })
    await expect(dialog.getByTestId('close-noncash')).toContainText('Безнал по счетам')
    await expect(dialog.locator('#close-counted')).toHaveAttribute('inputmode', 'none')
    const submit = dialog.getByRole('button', { name: 'Закрыть', exact: true }).last()
    expect((await submit.boundingBox())!.height).toBeGreaterThanOrEqual(56)
    await submit.tap()
    const keep = page.getByRole('button', { name: 'Деньги остались в кассе' })
    expect((await keep.boundingBox())!.height).toBeGreaterThanOrEqual(56)
    await keep.tap()
    await expect(bar).toContainText('Смена не открыта')
    await expect.poll(async () => (await prints(page)).filter((t) => t.includes(`Чек смены № ${shift.number}`)).length).toBe(2)
    await page.evaluate(() => localStorage.removeItem('avtodom.cashier_mode'))
  })
})
