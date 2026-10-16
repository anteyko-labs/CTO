// «Обзор счетов» в браузере (SPEC-28 §5, ADR-076, ADR-077): владелец заводит банк со своей ставкой
// комиссии, кассир продаёт картой на этот банк, счёт виден в обзоре с остатком и в истории — с
// продажей и комиссией банка; ставка меняется в карточке счёта с историей.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { pickCashier } from './cashier'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2ea' }
const RUN = Date.now().toString().slice(-8)

/** Сумма так, как её пишет экран: «1 234 с», «12,50 с»; пробелы — любые. */
function money(tyiyn: number): RegExp {
  const sign = tyiyn < 0 ? '−' : ''
  const abs = Math.abs(tyiyn)
  const whole = String(Math.trunc(abs / 100)).replace(/\B(?=(\d{3})+(?!\d))/g, ' ')
  const frac = abs % 100 ? `,${String(abs % 100).padStart(2, '0')}` : ''
  return new RegExp(`${sign}${whole}${frac}\\s*с`.replace(/ /g, '\\s'))
}

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'GET' | 'POST' | 'PATCH', path: string, data?: unknown): Promise<T> {
  const res = await request.fetch(`/api/v1${path}`, { method, data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

interface Account {
  id: string
  name: string
  kind: string
  default_transfer?: boolean
  fee_rate_bp?: number
  balance_tyiyn: number
}

test('свой банк: завести, продать картой на него, увидеть в обзоре и истории', async ({ page }) => {
  await login(page)
  const bankName = `Оптима ${RUN}`
  const cashier = `Кассир счетов ${RUN}`
  await api(page.request, 'POST', '/employees', { full_name: cashier, is_cashier: true })
  const cat = await api<{ id: string }>(page.request, 'POST', '/categories', { name: `Счета ${RUN}`, kind: 'other' })
  const code = `29${RUN}555`
  const item = `Ветошь ${RUN}`
  const p = await api<{ id: string }>(page.request, 'POST', '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: item,
    sale_price_tyiyn: 100_000,
    barcodes: [code],
  })
  await api(page.request, 'POST', '/receipts', { op_id: crypto.randomUUID(), paid_from: 'outside', lines: [{ product_id: p.id, qty: 3, cost_tyiyn: 150_000 }] })

  // «+ Счёт»: безналичный, комиссия 1 %; минус по факту разрешён по умолчанию.
  await page.getByRole('link', { name: 'Счета', exact: true }).first().click()
  await expect(page.getByRole('heading', { name: 'Обзор счетов' })).toBeVisible()
  await page.getByRole('button', { name: 'Счёт', exact: true }).click()
  await page.locator('#account-name').fill(bankName)
  await page.locator('#account-kind').selectOption('bank')
  await page.locator('#account-fee').fill('1')
  await page.getByRole('button', { name: 'Сохранить' }).click()
  await expect(page.getByText('Счёт заведён')).toBeVisible()
  const row = page.getByTestId('account-row').filter({ hasText: bankName })
  await expect(row).toContainText('Безналичный')
  await expect(row).toContainText('Разрешён')
  await expect(row).toContainText('1,00 %')
  await expect(row).toContainText(money(0))

  // Касса: картой, банк — новый счёт.
  await page.goto('/')
  await pickCashier(page, cashier)
  const picker = page.getByPlaceholder(/Сканируйте штрихкод/)
  await picker.fill(code)
  await picker.press('Enter')
  await expect(page.getByText(item)).toBeVisible()
  await page.locator('[data-pay]').getByRole('button', { name: 'Карта', exact: true }).click()
  await page.getByTestId('pay-bank').getByRole('button', { name: bankName }).click()
  await page.getByRole('button', { name: 'Провести чек' }).click()
  await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()

  // Деньги и комиссия банка — на выбранном счёте, сервер считает остаток.
  const accounts = await api<Account[]>(page.request, 'GET', '/cash/accounts')
  const bank = accounts.find((a) => a.name === bankName)
  expect(bank).toBeTruthy()
  // 1 000 с картой, комиссия 1 % — 10 с.
  expect(bank?.balance_tyiyn).toBe(99_000)

  // Обзор: остаток счёта и итог «наличные · безнал · всего».
  await page.getByRole('link', { name: 'Счета', exact: true }).first().click()
  await expect(row).toContainText(money(bank?.balance_tyiyn ?? 0))
  const bankTotal = accounts.filter((a) => a.kind === 'bank').reduce((s, a) => s + a.balance_tyiyn, 0)
  await expect(page.getByTestId('account-totals')).toContainText(money(bankTotal))

  // История: продажа и комиссия банка, на конец — остаток.
  await row.getByRole('button', { name: `История «${bankName}»` }).click()
  const dialog = page.getByRole('dialog', { name: `История: ${bankName}` })
  await expect(dialog.getByTestId('account-movement')).toHaveCount(2)
  await expect(dialog.getByTestId('account-movement').filter({ hasText: 'Комиссия банка' })).toHaveCount(1)
  await expect(dialog.getByTestId('account-history-totals')).toContainText(money(bank?.balance_tyiyn ?? 0))
  await dialog.getByRole('button', { name: 'Закрыть' }).click()

  // Ставка с завтрашнего дня — в истории ставок карточки счёта; сегодня действует прежняя.
  await row.getByRole('button', { name: `Изменить «${bankName}»` }).click()
  const tomorrow = new Date(Date.now() + 30 * 3600_000).toISOString().slice(0, 10)
  await page.locator('#account-fee').fill('1,5')
  await page.locator('#account-fee-from').fill(tomorrow)
  await page.getByRole('button', { name: 'Сохранить' }).click()
  await expect(page.getByText('Счёт сохранён')).toBeVisible()
  await expect(row).toContainText('1,00 %')
  await row.getByRole('button', { name: `Изменить «${bankName}»` }).click()
  await expect(page.getByTestId('fee-history')).toContainText('1,50 %')
  await expect(page.getByTestId('fee-history')).toContainText('1,00 %')
  await page.getByRole('button', { name: 'Отмена' }).click()

  // Убрать за собой: деньги — на счёт по умолчанию для перевода, банк — в архив (история остаётся).
  const main = accounts.find((a) => a.kind === 'bank' && a.default_transfer)
  await api(page.request, 'POST', '/cash/transfers', {
    op_id: crypto.randomUUID(),
    from_account_id: bank?.id,
    to_account_id: main?.id,
    amount_tyiyn: bank?.balance_tyiyn,
  })
  await page.reload()
  await row.getByRole('button', { name: `Изменить «${bankName}»` }).click()
  await page.getByText('В архиве (только с нулевым остатком)').click()
  await page.getByRole('button', { name: 'Сохранить' }).click()
  await expect(row).toHaveCount(0)
  await page.getByText('Показать архивные').click()
  await expect(row).toContainText('в архиве')
})
