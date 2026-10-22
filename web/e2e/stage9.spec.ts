// Деньги и отчёты в браузере (SPEC-27 §6): сводка за любой день и период, экран «Счёт».
import { expect, type APIRequestContext, type Page } from '@playwright/test'
import { ownerSafe } from './safe'
import { test } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2e9' }
const RUN = Date.now().toString().slice(-8)

/** Сумма так, как её пишет экран: «1 234 с», «12,50 с»; пробелы — любые. */
function money(tyiyn: number): RegExp {
  const sign = tyiyn < 0 ? '−' : ''
  const abs = Math.abs(tyiyn)
  const whole = String(Math.trunc(abs / 100)).replace(/\B(?=(\d{3})+(?!\d))/g, ' ')
  const frac = abs % 100 ? `,${String(abs % 100).padStart(2, '0')}` : ''
  return new RegExp(`${sign}${whole}${frac} с`.replace(/ /g, '\\s'))
}

/** Дата YYYY-MM-DD по Бишкеку со сдвигом и она же как «16.10.2026». */
function day(shift: number): [string, string] {
  const today = new Date().toLocaleDateString('sv-SE', { timeZone: 'Asia/Bishkek' })
  const d = new Date(`${today}T12:00:00Z`)
  d.setUTCDate(d.getUTCDate() + shift)
  const iso = d.toISOString().slice(0, 10)
  return [iso, `${iso.slice(8, 10)}.${iso.slice(5, 7)}.${iso.slice(0, 4)}`]
}

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
  const res = await request.get(`/api/v1${path}`, { headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

/** Товар с остатком и кассир; чек картой и переводом — деньги на «Счёт». */
async function sellNoncash(request: APIRequestContext) {
  const cat = await apiPost<{ id: string }>(request, '/categories', { name: `Отчёты ${RUN}`, kind: 'other' })
  const p = await apiPost<{ id: string }>(request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Щётка ${RUN}`,
    sale_price_tyiyn: 50_000,
  })
  await apiPost(request, '/receipts', {
    op_id: crypto.randomUUID(),
    paid_from: await ownerSafe(request),
    lines: [{ product_id: p.id, qty: 4, cost_tyiyn: 80_000 }],
  })
  const cashier = await apiPost<{ id: string }>(request, '/employees', { full_name: `Кассир отчётов ${RUN}`, is_cashier: true, pin: '1234' })
  await apiPost(request, '/sales', {
    op_id: crypto.randomUUID(),
    sale_type: 'takeaway',
    cashier_id: cashier.id,
    lines: [{ kind: 'piece', product_id: p.id, qty: 2, unit_price_tyiyn: 50_000 }],
    payments: [
      { method: 'card', amount_tyiyn: 60_000 },
      { method: 'transfer', amount_tyiyn: 40_000 },
    ],
  })
}

interface Totals {
  goods_tyiyn: number
  services_tyiyn: number
}

test('сводка: любой день и произвольный период', async ({ page }) => {
  await login(page)
  await sellNoncash(page.request)
  await page.goto('/summary')
  await expect(page.getByRole('heading', { name: 'Сводка сегодня' })).toBeVisible()
  // «Сколько по карте ушло» — безнал чеков за сегодня, как считает сервер.
  const dash = await apiGet<{ noncash: { card_tyiyn: number; total_tyiyn: number } }>(page.request, '/owner/dashboard')
  await expect(page.getByTestId('noncash')).toContainText(money(dash.noncash.total_tyiyn))
  await expect(page.getByTestId('noncash')).toContainText(money(dash.noncash.card_tyiyn))

  // Любой день: вчера — итоги как в отчёте о прибыли за вчера.
  const [yIso, yText] = day(-1)
  await page.getByLabel('День сводки').fill(yIso)
  await expect(page.getByRole('heading', { name: `Сводка за ${yText}` })).toBeVisible()
  const yProfit = await apiGet<{ totals: Totals }>(page.request, `/reports/profit?from=${yIso}&to=${yIso}`)
  const sold = page.getByText('Продано', { exact: true }).locator('..')
  await expect(sold).toContainText(money(yProfit.totals.goods_tyiyn + yProfit.totals.services_tyiyn))
  await expect(page.getByRole('heading', { name: `Продажи за ${yText} по часам` })).toBeVisible()

  // Произвольный период: с 10 дней назад по сегодня.
  await page.getByRole('button', { name: 'Период' }).click()
  const [fromIso, fromText] = day(-10)
  const [toIso, toText] = day(0)
  await page.getByLabel('Период по').fill(toIso)
  await page.getByLabel('Период с').fill(fromIso)
  await expect(page.getByRole('heading', { name: `Сводка с ${fromText} по ${toText}` })).toBeVisible()
  const pProfit = await apiGet<{ totals: Totals }>(page.request, `/reports/profit?from=${fromIso}&to=${toIso}`)
  await expect(sold).toContainText(money(pProfit.totals.goods_tyiyn + pProfit.totals.services_tyiyn))
  await expect(page.getByRole('heading', { name: new RegExp(`Прибыль по дням с ${fromText}`) })).toBeVisible()

  // Назад к «Сегодня» одной кнопкой.
  await page.getByRole('button', { name: 'Сегодня' }).click()
  await expect(page.getByRole('heading', { name: 'Сводка сегодня' })).toBeVisible()
})

test('«Счёт»: поступления картой и переводом сходятся с движениями', async ({ page }) => {
  await login(page)
  await sellNoncash(page.request)
  await page.getByRole('link', { name: 'Безнал (банк)', exact: true }).first().click()
  await expect(page.getByRole('heading', { name: 'Счёт', exact: true })).toBeVisible()
  const bank = await apiGet<{
    card: { total_tyiyn: number }
    transfer: { total_tyiyn: number }
    fee_tyiyn: number
    balance_tyiyn: number
    movements: { amount_tyiyn: number }[]
    movements_count: number
  }>(page.request, '/cash/bank')
  expect(bank.card.total_tyiyn).toBeGreaterThanOrEqual(60_000)
  const tile = (label: string) => page.getByText(label, { exact: true }).first().locator('..')
  await expect(tile('Карта / QR банка')).toContainText(money(bank.card.total_tyiyn))
  await expect(tile('QR владельцу')).toContainText(money(bank.transfer.total_tyiyn))
  await expect(tile('Комиссия банка')).toContainText(money(bank.fee_tyiyn))
  await expect(tile('Остаток сейчас')).toContainText(money(bank.balance_tyiyn))
  await expect(page.getByText(`Движения: ${bank.movements_count}`)).toBeVisible()
  // Движения идут страницами по 50 (SPEC-28 §2): на первой — первые 50, на последней — остаток,
  // а вместе страницы дают все движения за период.
  const PAGE = 50
  const all = bank.movements.length
  await expect(page.getByTestId('bank-movement')).toHaveCount(Math.min(all, PAGE))
  if (all > PAGE) {
    const pager = page.getByRole('navigation', { name: 'Страницы движений' })
    await expect(pager.getByTestId('pager-range')).toHaveText(`1–${PAGE} / ${all}`)
    await pager.getByRole('button', { name: 'Последняя страница' }).click()
    const lastStart = Math.floor((all - 1) / PAGE) * PAGE
    await expect(pager.getByTestId('pager-range')).toHaveText(`${lastStart + 1}–${all} / ${all}`)
    await expect(page.getByTestId('bank-movement')).toHaveCount(all - lastStart)
  }
})
