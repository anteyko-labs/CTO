// Карточка товара из строки ревизии (задача 002, П5; SPEC-15): владелец правит штрихкод и название —
// строка ревизии обновляется, остаток не меняется (I6); администратор без права «Товары и штрихкоды»
// карточку только смотрит, сервер правку не примет.
import { expect, test, type APIRequestContext, type Browser, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e002' }
const RUN = Date.now().toString().slice(-8)

async function login(page: Page, user: string, password: string) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(user)
  await page.getByLabel('Пароль').fill(password)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'get' | 'post' | 'put', path: string, data?: unknown): Promise<T> {
  const res = await request[method](`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

interface Line {
  product_id: string
  name: string
  barcodes: string[]
  expected_qty: number
}

/** Категория с одним товаром на остатке 3 шт и черновик ревизии по ней. */
async function world(request: APIRequestContext, tag: string) {
  const catName = `Ревизия карточки ${tag} ${RUN}`
  const cat = await api<{ id: string }>(request, 'post', '/categories', { name: catName, kind: 'other' })
  const name = `Свеча ${tag} ${RUN}`
  const code = `47${RUN}${tag === 'а' ? '01' : '02'}`
  const p = await api<{ id: string }>(request, 'post', '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name,
    barcodes: [code],
    sale_price_tyiyn: 50_000,
  })
  const supplier = await api<{ id: string }>(request, 'post', '/suppliers', { name: `Поставщик свечей ${tag} ${RUN}` })
  await api(request, 'post', '/receipts', {
    op_id: crypto.randomUUID(),
    supplier_id: supplier.id,
    payment: 'debt',
    lines: [{ product_id: p.id, qty: 3, cost_tyiyn: 90_000 }],
  })
  return { catName, catId: cat.id, name, code, id: p.id }
}

const line = async (request: APIRequestContext, revisionId: string, productId: string) =>
  (await api<{ lines: Line[] }>(request, 'get', `/revisions/${revisionId}`)).lines.find((l) => l.product_id === productId)!

test('владелец открывает карточку из строки ревизии и правит штрихкод; остаток не меняется', async ({ page }) => {
  await login(page, OWNER, OWNER_PASSWORD)
  const w = await world(page.request, 'а')
  await page.goto('/revision')
  await page.getByLabel('Что пересчитываем').selectOption({ label: w.catName })
  await page.getByRole('button', { name: 'Начать ревизию' }).click()
  await expect(page.getByRole('heading', { name: /Ревизия № \d+/ })).toBeVisible()
  const revs = await api<{ id: string; status: string; category_id: string | null }[]>(page.request, 'get', '/revisions')
  const rev = revs.find((r) => r.status === 'draft' && r.category_id === w.catId)!

  const row = page.getByRole('row', { name: new RegExp(w.name) })
  await expect(row.getByTestId('expected')).toHaveText('3 шт')
  await row.getByTestId('revision-card').click()
  const dialog = page.getByRole('dialog', { name: 'Товар' })
  await expect(dialog.locator('#product-name')).toHaveValue(w.name)
  // Остаток в карточке не правится; «Объединить дубли» (перенос остатка) во время ревизии скрыто.
  await expect(dialog.getByTestId('revision-stock-note')).toBeVisible()
  await expect(dialog.getByRole('button', { name: 'Объединить дубли' })).toHaveCount(0)
  // Новый штрихкод и название.
  const extra = `48${RUN}01`
  await dialog.locator('#product-barcode').fill(extra)
  await dialog.getByRole('button', { name: 'Добавить', exact: true }).click()
  await expect(dialog.getByText(extra, { exact: true })).toBeVisible()
  await dialog.locator('#product-name').fill(`${w.name} новая`)
  await dialog.getByRole('button', { name: 'Сохранить' }).click()
  await expect(page.getByText('Товар сохранён')).toBeVisible()
  await expect(dialog).toHaveCount(0)

  // Строка ревизии — с новым названием и кодом: скан нового кода считает её.
  const renamed = page.getByRole('row', { name: new RegExp(`${w.name} новая`) })
  await expect(renamed).toBeVisible()
  const l = await line(page.request, rev.id, w.id)
  expect(l.name).toBe(`${w.name} новая`)
  expect(l.barcodes).toContain(extra)
  expect(l.expected_qty).toBe(3)
  const scanner = page.getByPlaceholder('Сканируйте штрихкод')
  await scanner.fill(extra)
  await scanner.press('Enter')
  // Скан прибавляет к общему итогу строки (задача 003, Н14): итог — рядом с полем.
  await expect(page.locator(`[data-total="${w.id}"]`)).toHaveText(/итого 1\sшт/)
  await expect(renamed.getByTestId('expected')).toHaveText('3 шт')
  const product = await api<{ stock_qty: number }>(page.request, 'get', `/products/${w.id}`)
  expect(product.stock_qty).toBe(3)
  await page.getByRole('button', { name: 'Отменить ревизию' }).click()
  await expect(page.getByRole('button', { name: 'Отменить ревизию' })).toHaveCount(0)
})

async function adminPage(browser: Browser, user: string, password: string) {
  const ctx = await browser.newContext()
  const page = await ctx.newPage()
  await login(page, user, password)
  return { ctx, page }
}

test('администратор без права «Товары и штрихкоды» карточку из ревизии только смотрит', async ({ page, browser }) => {
  await login(page, OWNER, OWNER_PASSWORD)
  const w = await world(page.request, 'б')
  const login_ = `revcard${RUN}`
  const password = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  const u = await api<{ id: string }>(page.request, 'post', '/users', { login: login_, password, full_name: `Админ ревизии ${RUN}`, role: 'admin' })
  const rev = await api<{ head: { id: string; number: number } }>(page.request, 'post', '/revisions', { category_id: w.catId })

  // Права по умолчанию — без «Товары и штрихкоды».
  const { ctx, page: a } = await adminPage(browser, login_, password)
  const me = await api<{ permissions: string[] }>(a.request, 'get', '/auth/me')
  expect(me.permissions).toContain('products')
  expect(me.permissions).toContain('revision')
  await api(page.request, 'put', `/users/${u.id}/permissions`, {
    op_id: crypto.randomUUID(),
    // Код к товару открывает и право кассы «Привязывать коды на кассе» (ADR-168) — снято тоже.
    permissions: me.permissions.filter((p) => p !== 'products' && p !== 'till_barcode'),
  })
  await a.goto('/revision')
  await a.getByRole('row', { name: new RegExp(w.catName) }).first().click()
  await expect(a.getByRole('heading', { name: new RegExp(`Ревизия № ${rev.head.number}`) })).toBeVisible()
  await a.getByRole('row', { name: new RegExp(w.name) }).getByTestId('revision-card').click()
  const dialog = a.getByRole('dialog', { name: 'Товар' })
  await expect(dialog.locator('#product-name')).toHaveValue(w.name)
  await expect(dialog.locator('#product-name')).toBeDisabled()
  await expect(dialog.locator('#product-barcode')).toBeDisabled()
  await expect(dialog.getByRole('button', { name: 'Сохранить' })).toHaveCount(0)
  await expect(dialog.getByRole('button', { name: 'Добавить', exact: true })).toHaveCount(0)
  await expect(dialog.getByText('Править товары — право «Товары и штрихкоды»')).toBeVisible()
  await dialog.getByRole('button', { name: 'Отмена' }).click()
  await expect(dialog).toHaveCount(0)
  // Скрытая кнопка — не защита: сервер правку не примет (I12).
  const denied = await a.request.patch(`/api/v1/products/${w.id}`, { data: { name: 'чужое', category_id: w.catId }, headers: DEVICE })
  expect(denied.status()).toBe(403)
  expect(((await denied.json()) as { error: { message: string } }).error.message).toBe('Нет права: Товары и штрихкоды')
  const barcode = await a.request.post(`/api/v1/products/${w.id}/barcodes`, { data: { code: `49${RUN}01` }, headers: DEVICE })
  expect(barcode.status()).toBe(403)
  expect(((await barcode.json()) as { error: { message: string } }).error.message).toBe('Нет права: Привязывать коды на кассе')
  await ctx.close()
  await api(page.request, 'post', `/revisions/${rev.head.id}/cancel`, {})
})
