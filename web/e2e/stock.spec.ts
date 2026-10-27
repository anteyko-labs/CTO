// «Склад → Остатки» (SPEC-36 §14, ADR-125): только ненулевые остатки, минус — «проверить», группы по
// категориям с подытогами, итог склада, поиск, отбор по категории, печать листа; администратору —
// без закупки. Старые ссылки «Остатков» с отбором ведут в «Товары».
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e361' }
const RUN = Date.now().toString().slice(-8)
const money = (text: string) => new RegExp(text.replace(/ /g, '\\s'))

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
    h.replace(/<style[\s\S]*?<\/style>/g, '').replace(/<[^>]+>/g, ' ').replace(/&nbsp;| /g, ' ').replace(/\s+/g, ' '),
  )

test('«Склад → Остатки»: ненулевые остатки по категориям, подытоги, минус, поиск, печать; администратору без закупки', async ({ page, browser }) => {
  await page.addInitScript(PRINT_STUB)
  await login(page)
  const oils = await apiPost<{ id: string }>(page.request, '/categories', { name: `Масла остатков ${RUN}`, kind: 'oil' })
  const lamps = await apiPost<{ id: string }>(page.request, '/categories', { name: `Лампы остатков ${RUN}`, kind: 'other' })
  const product = (body: Record<string, unknown>) => apiPost<{ id: string }>(page.request, '/products', { op_id: crypto.randomUUID(), ...body })
  const oil = await product({ category_id: oils.id, name: `Масло остатков 5W-30 ${RUN}`, container_ml: 4000, sale_price_tyiyn: 200_000, attrs: { viscosity: '5W-30' } })
  const oil2 = await product({ category_id: oils.id, name: `Масло остатков 10W40 ${RUN}`, container_ml: 4000, sale_price_tyiyn: 240_000 })
  await product({ category_id: oils.id, name: `Масло пустое ${RUN}`, container_ml: 4000, sale_price_tyiyn: 100_000 })
  const lamp = await product({ category_id: lamps.id, name: `Лампа остатков ${RUN}`, sale_price_tyiyn: 10_000 })
  // 8 кан. + 2,5 л и 1 канистра — оприходованием; лампу продали без остатка — минус.
  await apiPost(page.request, '/stock-ins', {
    op_id: crypto.randomUUID(),
    comment: `остатки ${RUN}`,
    lines: [
      { product_id: oil.id, qty: 34_500, price_tyiyn: 150_000 },
      { product_id: oil2.id, qty: 4_000, price_tyiyn: 180_000 },
    ],
  })
  const cashier = await apiPost<{ id: string }>(page.request, '/employees', { full_name: `Кассир остатков ${RUN}`, is_cashier: true, pin: '1234' })
  await apiPost(page.request, '/sales', {
    op_id: crypto.randomUUID(),
    sale_type: 'takeaway',
    cashier_id: cashier.id,
    lines: [{ kind: 'piece', product_id: lamp.id, qty: 2, unit_price_tyiyn: 10_000 }],
    payments: [{ method: 'card', amount_tyiyn: 20_000 }],
  })

  // Пункт меню «Склад → Остатки».
  await page.getByRole('link', { name: 'Остатки', exact: true }).first().click()
  await expect(page).toHaveURL(/\/stock$/)
  await expect(page.getByRole('heading', { name: 'Остатки' })).toBeVisible()
  await page.getByTestId('stock-search').fill(RUN)
  const table = page.getByTestId('stock-table')
  await expect(table.getByTestId('stock-row')).toHaveCount(3)
  await expect(table).not.toContainText(`Масло пустое ${RUN}`)

  // Масло: литры и «кан. + л», вязкость из карточки и (серым) из названия.
  const oilRow = table.getByTestId('stock-row').filter({ hasText: `Масло остатков 5W-30 ${RUN}` })
  await expect(oilRow).toContainText('34,5 л · 8 кан. + 2,5 л')
  await expect(oilRow).toContainText('5W-30')
  await expect(oilRow).toContainText(money('17 250 с'))
  await expect(table.getByTestId('stock-row').filter({ hasText: `10W40 ${RUN}` })).toContainText('10W-40')
  // Минус — красным, «проверить».
  const lampRow = table.getByTestId('stock-row').filter({ hasText: `Лампа остатков ${RUN}` })
  await expect(lampRow).toContainText('проверить')
  await expect(lampRow).toContainText('-2 шт')
  await expect(lampRow).toHaveClass(/text-rose-700/)

  // Подытоги групп: позиции, количество, суммы; владельцу — и закупка.
  const subtotals = table.getByTestId('stock-subtotal')
  await expect(table.getByTestId('stock-group').first()).toContainText(`Лампы остатков ${RUN}`)
  await expect(subtotals.nth(1)).toContainText('Итого: 2 поз.')
  await expect(subtotals.nth(1)).toContainText('38,5 л · 9 кан. + 2,5 л')
  await expect(subtotals.nth(1)).toContainText(money('19 650 с'))
  await expect(subtotals.nth(1)).toContainText(money('14 737,50 с'))
  await expect(table.getByTestId('stock-total')).toContainText('Итого по складу: 3 поз.')
  await expect(table.getByTestId('stock-total')).toContainText(money('19 450 с'))

  // Отбор по категории.
  await page.getByTestId('stock-category').selectOption({ label: `Лампы остатков ${RUN}` })
  await expect(table.getByTestId('stock-row')).toHaveCount(1)
  await page.getByTestId('stock-category').selectOption({ label: 'Все категории' })
  await expect(table.getByTestId('stock-row')).toHaveCount(3)

  // Печать: лист остатков на дату с подытогами и закупкой.
  await page.getByTestId('stock-print').click()
  await expect.poll(async () => (await prints(page)).length).toBe(1)
  const sheet = (await prints(page))[0]
  expect(sheet).toMatch(/Остатки на \d\d\.\d\d\.\d{4}/)
  expect(sheet).toContain(`Масло остатков 5W-30 ${RUN}`)
  expect(sheet).toContain(`Лампа остатков ${RUN} (проверить)`)
  expect(sheet).toContain('Сумма (закупка)')
  expect(sheet).toContain('Итого по складу: 3 поз.')

  // Старая ссылка «Остатков» с отбором — в «Товары».
  await page.goto('/stock?filter=low')
  await expect(page).toHaveURL(/\/products\?stock=low/)

  // Администратор: без закупки на экране и в печати.
  const login_ = `stock${RUN}`
  const password = `pass-${RUN}-stock`
  await apiPost(page.request, '/users', { login: login_, password, full_name: `Админ остатков ${RUN}`, role: 'admin' })
  const ctx = await browser.newContext()
  const a = await ctx.newPage()
  await a.addInitScript(PRINT_STUB)
  await login(a, login_, password)
  await a.goto('/stock')
  await a.getByTestId('stock-search').fill(RUN)
  const t = a.getByTestId('stock-table')
  await expect(t.getByTestId('stock-row')).toHaveCount(3)
  await expect(t).not.toContainText('Сумма (закупка)')
  await expect(t.getByTestId('stock-subtotal').nth(1)).not.toContainText(money('14 737,50 с'))
  await a.getByTestId('stock-print').click()
  await expect.poll(async () => (await prints(a)).length).toBe(1)
  expect((await prints(a))[0]).not.toContain('закупка')
  await ctx.close()
})
