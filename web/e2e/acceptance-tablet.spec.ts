// Приёмка товара на планшете (SPEC-39, ADR-134): 1280 × 800 касаниями — отметки, кассир, PIN,
// подпись, приход проведён; обрыв связи и перезагрузка отметки не теряют; цели касания от 56.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e339' }
const RUN = Date.now().toString().slice(-8)

test.use({ viewport: { width: 1280, height: 800 }, hasTouch: true })

async function login(page: Page, user: string, password: string) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(user)
  await page.getByLabel('Пароль').fill(password)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Приёмка' })).toBeVisible()
}

async function apiPost<T>(request: APIRequestContext, path: string, data: unknown, method: 'post' | 'put' = 'post'): Promise<T> {
  const res = await request[method](`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

test('кассир принимает товар на планшете: отметки, PIN, подпись — приход по принятому', async ({ page, browser }) => {
  await login(page, OWNER, OWNER_PASSWORD)
  const req = page.request
  const supplier = await apiPost<{ id: string }>(req, '/suppliers', { name: `Автоснаб ${RUN}` })
  const cat = await apiPost<{ id: string }>(req, '/categories', { name: `Фильтры ${RUN}`, kind: 'filter' })
  const filter = await apiPost<{ id: string }>(req, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Фильтр W712 ${RUN}`,
    sale_price_tyiyn: 90_000,
  })
  const plug = await apiPost<{ id: string }>(req, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Свеча ${RUN}`,
    sale_price_tyiyn: 30_000,
  })
  const cashier = `Айбек ${RUN}`
  await apiPost(req, '/employees', { full_name: cashier, is_cashier: true, pin: '1234' })
  const tablet = `planshet${RUN}`
  const password = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  const u = await apiPost<{ id: string }>(req, '/users', { login: tablet, password, full_name: 'Планшет приёмки', role: 'admin' })
  await apiPost(req, `/users/${u.id}/permissions`, { op_id: crypto.randomUUID(), permissions: ['acceptance'] }, 'put')
  const doc = `П${RUN}`
  await apiPost(req, '/acceptances', {
    op_id: crypto.randomUUID(),
    supplier_id: supplier.id,
    supplier_doc: doc,
    lines: [
      { product_id: filter.id, qty: 20, cost_tyiyn: 100_000 },
      { product_id: plug.id, qty: 4, cost_tyiyn: 40_000 },
    ],
  })

  // Планшет: вход с одним правом «Приёмка товара», режим приёмки.
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 800 }, hasTouch: true })
  const t = await ctx.newPage()
  await login(t, tablet, password)
  await t.getByRole('link', { name: 'Приёмка' }).click()
  await t.getByTestId('acceptance-mode-on').tap()
  const card = t.getByTestId('acceptance-card').filter({ hasText: doc })
  await expect(card).toBeVisible()
  expect((await card.boundingBox())!.height).toBeGreaterThanOrEqual(56)
  await card.tap()

  const lines = t.getByTestId('acceptance-line')
  await expect(lines).toHaveCount(2)
  // Цен и сумм на экране нет.
  await expect(t.getByText('100 000')).toHaveCount(0)
  const first = lines.filter({ hasText: `Фильтр W712 ${RUN}` })
  const second = lines.filter({ hasText: `Свеча ${RUN}` })
  await first.getByTestId('acceptance-line-ok').tap()
  await expect(first).toHaveAttribute('data-state', 'ok')
  await second.getByTestId('acceptance-line-qty').fill('3')
  await expect(second).toHaveAttribute('data-state', 'less')
  await expect(second.getByText('3 из 4')).toBeVisible()
  await second.getByTestId('acceptance-line-reasons').getByRole('button', { name: 'брак или повреждено' }).tap()
  for (const b of await t.locator('main button:visible').all()) {
    const box = await b.boundingBox()
    if (box) expect(box.height, await b.innerText()).toBeGreaterThanOrEqual(56)
  }

  // Перезагрузка и обрыв связи отметок не теряют.
  await t.reload()
  await expect(lines.filter({ hasText: `Фильтр W712 ${RUN}` })).toHaveAttribute('data-state', 'ok')
  await expect(lines.filter({ hasText: `Свеча ${RUN}` })).toHaveAttribute('data-state', 'less')
  await ctx.setOffline(true)
  await expect(t.getByText(/Нет связи с сервером — приёмка недоступна/)).toBeVisible()
  await expect(t.getByTestId('acceptance-accept')).toBeDisabled()
  await ctx.setOffline(false)
  await expect(t.getByTestId('acceptance-accept')).toBeEnabled()

  // Принять: кассир, PIN, подпись, итог.
  await t.getByTestId('acceptance-accept').tap()
  await t.getByTestId('acceptance-cashier').filter({ hasText: cashier }).tap()
  await t.getByTestId('acceptance-pin').fill('1234')
  await t.getByTestId('acceptance-pin-next').tap()
  const pad = t.getByTestId('signature-pad').locator('canvas')
  await expect(t.getByTestId('signature-done')).toBeDisabled()
  const box = (await pad.boundingBox())!
  await t.mouse.move(box.x + 40, box.y + 40)
  await t.mouse.down()
  for (let i = 1; i <= 10; i++) await t.mouse.move(box.x + 40 + i * 30, box.y + 40 + (i % 2) * 60)
  await t.mouse.up()
  await t.getByTestId('signature-done').tap()
  await expect(t.getByTestId('acceptance-summary')).toHaveText('Принять 2 строки, расхождений 1')
  await t.getByTestId('acceptance-submit').tap()
  await expect(t.getByText(/Приход № \d+ проведён/)).toBeVisible()
  await expect(t.getByTestId('acceptance-card').filter({ hasText: doc })).toHaveCount(0)
  await ctx.close()

  // Владелец: в накладной — кассир, расхождение и подпись; сумма — за принятое.
  const list = (await (await req.get(`/api/v1/receipts?kind=receipt`)).json()) as { id: string; supplier_doc: string; total_tyiyn: number }[]
  const r = list.find((x) => x.supplier_doc === doc)
  expect(r, 'приход по приёмке').toBeTruthy()
  expect(r!.total_tyiyn).toBe(100_000 + 30_000)
  await page.goto(`/receipts/${r!.id}`)
  const box2 = page.getByTestId('receipt-acceptance')
  await expect(box2).toContainText(`Принято на планшете: ${cashier}`)
  await expect(box2).toContainText('брак или повреждено')
  await expect(page.getByTestId('receipt-acceptance-signature')).toBeVisible()
})

test('«Приход»: блок «Ждут приёмки» с суммой, «Убрать с приёмки» с причиной', async ({ page }) => {
  await login(page, OWNER, OWNER_PASSWORD)
  const req = page.request
  const supplier = await apiPost<{ id: string }>(req, '/suppliers', { name: `Запчасти ${RUN}` })
  const cat = await apiPost<{ id: string }>(req, '/categories', { name: `Щётки ${RUN}`, kind: 'other' })
  const p = await apiPost<{ id: string }>(req, '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name: `Щётка ${RUN}`, sale_price_tyiyn: 50_000 })
  const doc = `Б${RUN}`
  await apiPost(req, '/acceptances', { op_id: crypto.randomUUID(), supplier_id: supplier.id, supplier_doc: doc, lines: [{ product_id: p.id, qty: 2, cost_tyiyn: 60_000 }] })
  await page.goto('/receipts')
  const row = page.getByTestId('acceptance-waiting-row').filter({ hasText: doc })
  await expect(row).toContainText('600')
  await row.getByTestId('acceptance-cancel').click()
  await page.getByTestId('acceptance-cancel-reason').fill('ошибка в накладной')
  await page.getByTestId('acceptance-cancel-confirm').click()
  await expect(page.getByTestId('acceptance-waiting-row').filter({ hasText: doc })).toHaveCount(0)
})
