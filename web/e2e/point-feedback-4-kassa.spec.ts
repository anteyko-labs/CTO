// Доработки по проверке на точке, часть 4 (SPEC-30 §11–13): Enter в поиске товара берёт найденное
// по всему тексту, «Остатки» ведут на «Товары», окно оплаты долга открывается на клиенте чека,
// баллы у клиентов всего.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e304' }
const PICKER = /Сканируйте штрихкод/
const RUN = Date.now().toString().slice(-8)
/** Сумма как на экране: пробелы в разрядах и перед «с» неразрывные. */
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

/** Штучный товар с приходом `qty` штук и минимумом остатка. */
async function product(request: APIRequestContext, categoryId: string, name: string, opts: { qty?: number; min?: number; article?: string } = {}) {
  const p = await apiPost<{ id: string }>(request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: categoryId,
    name,
    article: opts.article ?? '',
    sale_price_tyiyn: 50_000,
    min_stock: opts.min ?? 0,
  })
  if ((opts.qty ?? 5) > 0) {
    await apiPost(request, '/receipts', {
      op_id: crypto.randomUUID(),
      paid_from: await ownerSafe(request),
      lines: [{ product_id: p.id, qty: opts.qty ?? 5, cost_tyiyn: 30_000 * (opts.qty ?? 5) }],
    })
  }
  return p
}

test('Enter в поиске не берёт товар по первым буквам: ждёт найденного по всему тексту', async ({ page }) => {
  // 21.10.2026: кассир набрал моторное масло, нажал Enter — в чек встал воздушный фильтр.
  await login(page)
  const tag = `Энтер${RUN}`
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Поиск ${RUN}`, kind: 'other' })
  // По набранному началу фильтр — первый (точный артикул), полному тексту отвечает только масло.
  const filter = `Воздушный фильтр ${tag}`
  await product(page.request, cat.id, filter, { article: tag })
  await product(page.request, cat.id, `${tag} масло моторное`)

  await page.goto('/')
  const picker = page.getByPlaceholder(PICKER)
  await picker.fill(tag)
  const found = page.locator('ul').filter({ has: page.getByRole('button', { name: new RegExp(`^${filter}`) }) })
  await expect(found.getByRole('button').first()).toContainText(filter)
  await expect(found.getByRole('button', { name: new RegExp(`^${tag} масло моторное`) })).toBeVisible()
  // Ответ на полный текст приходит не сразу: Enter нажат, пока на экране найденное по началу.
  const full = (url: URL) => url.pathname === '/api/v1/products' && url.searchParams.get('q') === `${tag} масло`
  await page.route(full, async (route) => {
    await new Promise((r) => setTimeout(r, 1500))
    await route.continue()
  })
  await picker.pressSequentially(' масло')
  await picker.press('Enter')
  const cart = page.getByTestId('cart')
  await expect(cart).not.toContainText(filter)
  await expect(cart).toContainText(`${tag} масло моторное`)
  await expect(cart).not.toContainText(filter)
  await page.unroute(full)

  // Найдено по всему тексту — Enter сразу берёт выделенное.
  await picker.fill(`фильтр ${tag}`)
  await expect(page.getByRole('button', { name: new RegExp(`^${filter}`) })).toBeVisible()
  await picker.press('Enter')
  await expect(cart).toContainText(filter)
  await page.getByRole('button', { name: 'Очистить', exact: true }).click()
  await page.getByRole('button', { name: 'Точно очистить?' }).click()
})

test('«Остатки» ведут на «Товары» с теми же отборами', async ({ page }) => {
  await login(page)
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Склад ${RUN}`, kind: 'other' })
  const low = `Склад ${RUN} лампа мало`
  const plenty = `Склад ${RUN} лампа много`
  await product(page.request, cat.id, low, { qty: 2, min: 5 })
  await product(page.request, cat.id, plenty, { qty: 10, min: 2 })

  // Пункта «Остатки» в меню нет; старый адрес с отбором — «Товары» с тем же отбором.
  await page.goto('/')
  await expect(page.getByRole('link', { name: 'Остатки' })).toHaveCount(0)
  await page.goto('/stock?filter=low')
  await expect(page).toHaveURL(/\/products\?stock=low$/)
  await expect(page.getByRole('heading', { name: 'Товары' })).toBeVisible()
  const chip = (name: RegExp) => page.getByRole('group', { name: 'Отбор по складу' }).getByRole('button', { name })
  await expect(chip(/^Ниже минимума/)).toHaveAttribute('aria-pressed', 'true')
  await page.getByLabel('Поиск', { exact: true }).fill(`Склад ${RUN}`)
  await expect(page.getByRole('row', { name: new RegExp(low) })).toContainText('ниже минимума')
  await expect(page.getByRole('row', { name: new RegExp(low) })).toContainText(/мин\.\s5\sшт/)
  await expect(page.getByRole('row', { name: new RegExp(plenty) })).toHaveCount(0)
  // Итоги — по отбору: по закупке (владельцу) и по продаже. 2 шт по 300 с и по 500 с.
  const totals = page.getByTestId('product-totals')
  await expect(totals).toContainText(/1\sтовар/)
  await expect(totals).toContainText(money('по закупке 600 с'))
  await expect(totals).toContainText(money('по продаже 1 000 с'))
  // Числа на кнопках отборов — при том же поиске.
  await expect(chip(/^Все/)).toContainText('2')
  await chip(/^Все/).click()
  await expect(page).toHaveURL(/\/products$/)
  await expect(page.getByRole('row', { name: new RegExp(plenty) })).toBeVisible()
  // Сортировка по остатку и сверка остатков — на месте.
  await page.getByRole('button', { name: 'Остаток' }).click()
  await expect(page.getByRole('row').nth(1)).toContainText(plenty)
  await page.getByRole('button', { name: 'Сверить остатки' }).click()
  await expect(page.getByText('Расхождений нет')).toBeVisible()
  await page.keyboard.press('Escape')
  // Вкладка «Брак» — тоже здесь, и старый адрес с ней.
  await page.goto('/stock?tab=defects')
  await expect(page).toHaveURL(/\/products\?tab=defects$/)
  await expect(page.getByRole('tab', { name: 'Брак' })).toHaveAttribute('aria-selected', 'true')
})

test('оплата долга на кассе открывается на клиенте чека, крупно', async ({ page }) => {
  await login(page)
  const cashier = await apiPost<{ id: string }>(page.request, '/employees', { full_name: `Кассир долга ${RUN}`, is_cashier: true, pin: '1234' })
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Долг ${RUN}`, kind: 'other' })
  const p = await product(page.request, cat.id, `Долг ${RUN} фильтр`)
  const client = `Должник Чеков ${RUN}`
  const party = await apiPost<{ id: string }>(page.request, '/parties', { role: 'customer', kind: 'person', name: client, inn: `4${RUN}12345` })
  await apiPost(page.request, '/sales', {
    op_id: crypto.randomUUID(),
    client_time: new Date().toISOString(),
    sale_type: 'takeaway',
    cashier_id: cashier.id,
    master_id: null,
    party_id: party.id,
    contact_id: null,
    vehicle_id: null,
    comment: '',
    lines: [{ kind: 'piece', gift: false, product_id: p.id, service_id: null, qty: 1, unit_price_tyiyn: 425_000 }],
    payments: [{ method: 'debt', amount_tyiyn: 425_000 }],
  })

  // Наличные принимаются в открытую смену.
  const current = await page.request.get('/api/v1/shifts/current')
  if (!(await current.json())) await apiPost(page.request, '/shifts/open', { op_id: crypto.randomUUID(), cashier_employee_id: cashier.id, cashier_pin: '1234' })

  await page.goto('/')
  await page.locator('[data-client-input]').fill(client)
  await page.getByRole('button', { name: new RegExp(`^${client}`) }).click()
  await expect(page.locator('[data-client]')).toContainText(money('долг 4 250 с'))
  // Клиент чека с долгом — окно сразу на нём, без поиска.
  await page.getByRole('button', { name: 'Принять оплату долга' }).click()
  await expect(page.getByRole('heading', { name: `Оплата долга: ${client}` })).toBeVisible()
  await expect(page.getByTestId('repay-debt')).toHaveText(money('4 250 с'))
  await expect(page.locator('#repay-sum')).toHaveValue('4250')
  // Сумма долга крупно.
  const big = await page.getByTestId('repay-debt').evaluate((el) => parseFloat(getComputedStyle(el).fontSize))
  expect(big).toBeGreaterThanOrEqual(28)
  // «Отмена» — к списку должников: строки крупные, клиент в нём с суммой.
  await page.getByRole('button', { name: 'Отмена' }).click()
  const debtors = page.getByTestId('debtors')
  await page.getByPlaceholder('Имя, телефон или ИНН должника').fill(client)
  await expect(debtors).toContainText(client)
  await expect(debtors).toContainText(money('4 250 с'))
  const row = debtors.getByRole('button', { name: new RegExp(client) })
  expect((await row.boundingBox())!.height).toBeGreaterThanOrEqual(56)
  // Оплата с кассы обновляет долг клиента в чеке.
  await row.click()
  await page.locator('#repay-sum').fill('250')
  await page.getByRole('button', { name: 'Принять оплату', exact: true }).click()
  await page.getByTestId('repay-result').getByRole('button', { name: 'Готово' }).click()
  await expect(page.locator('[data-client]')).toContainText(money('долг 4 000 с'))
})

test('баллы у клиентов всего — на экране «Клиенты»', async ({ page }) => {
  await login(page)
  await page.goto('/clients')
  const points = page.getByTestId('points-total')
  await expect(points).toContainText('Баллы у клиентов')
  await expect(points).toContainText(/на\s[\d\s]+(,\d\d)?\sс/)
})
