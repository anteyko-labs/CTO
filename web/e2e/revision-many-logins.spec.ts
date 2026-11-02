// Ревизию ведут несколько сотрудников, у каждого свой вход (задача 005, Н7): «я занёс 6, Алексей 4 —
// в итоге 10». Поле строки по умолчанию «+ добавить», у строки — кто сколько добавил; поиск по
// названию в ревизии предлагает и строки старой базы Умаг с «Завести».
import { expect, test, type APIRequestContext, type Browser, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const RUN = Date.now().toString().slice(-8)
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e7a1' }
/** Текст с неразрывными пробелами — как на экране. */
const text = (t: string) => new RegExp(t.replace(/[.*+?^${}()|[\]\\]/g, '\\$&').replace(/ /g, '\\s'))

async function login(page: Page, user: string, password: string) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(user)
  await page.getByLabel('Пароль').fill(password)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' }).first()).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'post' | 'put', path: string, data: unknown): Promise<T> {
  const res = await request[method](`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

/** Сотрудник со своим входом (роль администратора, права по умолчанию — с «Ревизией»), в своём браузере. */
async function staff(owner: Page, browser: Browser, name: string, permissions?: (all: string[]) => string[]): Promise<Page> {
  const login_ = `rev${RUN}${Math.random().toString(36).slice(2, 6)}`
  const password = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  const u = await api<{ id: string }>(owner.request, 'post', '/users', {
    op_id: crypto.randomUUID(),
    login: login_,
    password,
    full_name: `${name} ${RUN}`,
    role: 'admin',
  })
  const page = await (await browser.newContext()).newPage()
  await login(page, login_, password)
  if (permissions) {
    const me = (await (await page.request.get('/api/v1/auth/me')).json()) as { permissions: string[] }
    await api(owner.request, 'put', `/users/${u.id}/permissions`, { op_id: crypto.randomUUID(), permissions: permissions(me.permissions) })
    await page.reload()
  }
  return page
}

async function openRevision(page: Page, category: string) {
  await page.goto('/revision')
  await page.getByRole('row', { name: new RegExp(category) }).first().click()
  await expect(page.getByRole('heading', { name: new RegExp(`Ревизия № \\d+ · ${category}`) })).toBeVisible()
}

async function cancelDraft(request: APIRequestContext, categoryId: string) {
  const list = (await (await request.get('/api/v1/revisions')).json()) as { id: string; status: string; category_id: string | null }[]
  for (const r of list.filter((x) => x.status === 'draft' && x.category_id === categoryId)) await api(request, 'post', `/revisions/${r.id}/cancel`, {})
}

const total = (page: Page, id: string) => page.locator(`[data-total="${id}"]`)

test('ревизия двумя входами: Азиз вписал 6, Алексей 4 — каждый видит своё, владелец — 10 и кто сколько; «= моё» исправляет своё', async ({ page, browser }) => {
  await login(page, OWNER, OWNER_PASSWORD)
  const category = `Кат два входа ${RUN}`
  const cat = await api<{ id: string }>(page.request, 'post', '/categories', { name: category, kind: 'other' })
  const name = `Фильтр два входа ${RUN}`
  const p = await api<{ id: string }>(page.request, 'post', '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name, sale_price_tyiyn: 30_000 })
  const missing = await api<{ id: string }>(page.request, 'post', '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Свеча нет на полке ${RUN}`,
    sale_price_tyiyn: 10_000,
  })
  await api(page.request, 'post', '/revisions', { category_id: cat.id })
  const aziz = await staff(page, browser, 'Азиз')
  const aleksey = await staff(page, browser, 'Алексей')
  await openRevision(aziz, category)
  await openRevision(aleksey, category)

  // Каждый видит только свой пересчёт (задача 007): поле — «+ добавить» без выбора, под ним — «моё».
  for (const [who, qty] of [
    [aziz, '6'],
    [aleksey, '4'],
  ] as const) {
    const row = who.getByRole('row', { name: new RegExp(name) })
    await expect(row.locator('[data-mode="add"]')).toHaveAttribute('aria-pressed', 'true')
    // Слепой пересчёт: учётного остатка сотрудник не видит.
    await expect(row.getByTestId('expected')).toHaveCount(0)
    const field = who.locator(`[data-count="${p.id}"]`)
    await field.fill(qty)
    await field.press('Enter')
    await expect(total(who, p.id)).toHaveText(text(`моё ${qty} шт`), { timeout: 15_000 })
    await expect(field).toHaveValue('')
  }
  // Чужое не видно и после перечитывания: у Азиза — его 6, «кто сколько» сотрудникам не показывается.
  await openRevision(aziz, category)
  await expect(total(aziz, p.id)).toHaveText(text('моё 6 шт'), { timeout: 15_000 })
  await expect(aziz.getByTestId('counted-by')).toHaveCount(0)
  // Владелец: в «Моё» строки не считал, в «Все вместе» — итог 10 и кто сколько.
  await openRevision(page, category)
  await page.locator('[data-view="own"]').click()
  await expect(total(page, p.id)).toHaveCount(0)
  await page.locator('[data-view="all"]').click()
  await expect(total(page, p.id)).toHaveText(text('итого 10 шт'), { timeout: 15_000 })
  await expect(page.getByRole('row', { name: new RegExp(name) }).getByTestId('counted-by')).toHaveText(
    text(`Азиз ${RUN} +6 шт, Алексей ${RUN} +4 шт = 10 шт`),
    { timeout: 15_000 },
  )
  // Азиз пробил лишнее: «= моё 3» — у него 3, у Алексея по-прежнему 4, итог 7.
  const row = aziz.getByRole('row', { name: new RegExp(name) })
  await row.locator('[data-mode="mine"]').click()
  const field = aziz.locator(`[data-count="${p.id}"]`)
  await expect(field).toHaveValue('6')
  await field.fill('3')
  await field.press('Enter')
  await expect(total(aziz, p.id)).toHaveText(text('моё 3 шт'), { timeout: 15_000 })
  await openRevision(aleksey, category)
  await expect(total(aleksey, p.id)).toHaveText(text('моё 4 шт'), { timeout: 15_000 })
  await openRevision(page, category)
  await expect(total(page, p.id)).toHaveText(text('итого 7 шт'), { timeout: 15_000 })
  // 0 в «+ добавить» непосчитанной строки — «не нашли»: у Азиза — 0, у владельца — посчитано 0.
  const zero = aziz.locator(`[data-count="${missing.id}"]`)
  await zero.fill('0')
  await zero.press('Enter')
  await expect(total(aziz, missing.id)).toHaveText(text('моё 0 шт'), { timeout: 15_000 })
  await openRevision(page, category)
  await expect(total(page, missing.id)).toHaveText(text('итого 0 шт'), { timeout: 15_000 })
  await cancelDraft(page.request, cat.id)
})

test('ревизия: поиск по названию — строка Умага — «Завести» — товар в ревизии с найденным; без права «Товары» — только заведённые', async ({
  page,
  browser,
}) => {
  await login(page, OWNER, OWNER_PASSWORD)
  const category = `Кат Умаг поиск ${RUN}`
  const cat = await api<{ id: string }>(page.request, 'post', '/categories', { name: category, kind: 'other' })
  const umagName = `Антифриз Умаг поиск ${RUN}`
  const umagCode = `61${RUN}01`
  await api(page.request, 'put', '/old-catalog', [{ name: umagName, barcode: umagCode, price_tyiyn: 50_000, cost_tyiyn: 30_000, unit: 'шт' }])
  await api(page.request, 'put', '/settings/old-catalog', { enabled: true })
  await api(page.request, 'post', '/revisions', { category_id: cat.id })
  const aziz = await staff(page, browser, 'Азиз')
  const noRight = await staff(page, browser, 'Эрмек', (all) => all.filter((x) => x !== 'products'))
  await openRevision(aziz, category)
  await openRevision(noRight, category)

  // Без права «Товары и штрихкоды» строк Умага в поиске нет.
  await noRight.getByPlaceholder('Название, артикул или вязкость').fill(`Антифриз Умаг поиск ${RUN}`)
  await noRight.waitForTimeout(1_000)
  await expect(noRight.getByTestId('revision-umag')).toHaveCount(0)

  await aziz.getByPlaceholder('Название, артикул или вязкость').fill(`Антифриз Умаг поиск ${RUN}`)
  const block = aziz.getByTestId('revision-umag')
  await expect(block).toContainText('Из старой базы Умаг')
  const hit = block.locator('[data-umag-row]', { hasText: umagName })
  await hit.getByRole('button', { name: 'Завести' }).click()
  const dialog = aziz.getByRole('dialog', { name: 'Товар, которого нет в программе' })
  await expect(dialog.locator('#revnew-name')).toHaveValue(umagName)
  await expect(dialog.getByText(`Данные из старой программы: ${umagName}`)).toBeVisible()
  await expect(dialog.locator('#revnew-count')).toBeFocused()
  await dialog.locator('#revnew-count').fill('3')
  await dialog.getByRole('button', { name: 'Завести и посчитать' }).click()
  await expect(dialog).toBeHidden()

  // Товар заведён, связан со строкой Умага и посчитан: у второго входа — тот же итог.
  const hits = (await (await page.request.get(`/api/v1/old-catalog/search?barcode=${umagCode}`)).json()) as { exact: boolean; product_id: string | null }[]
  const linked = hits.find((h) => h.exact)?.product_id
  expect(linked).toBeTruthy()
  await expect(total(aziz, linked!)).toHaveText(text('моё 3 шт'), { timeout: 15_000 })
  await noRight.getByPlaceholder('Название, артикул или вязкость').fill('')
  await expect(total(noRight, linked!)).toHaveText(text('моё 3 шт'), { timeout: 15_000 })
  // Строка заведена — в Умаге её больше не предлагают.
  await aziz.getByPlaceholder('Название, артикул или вязкость').fill(`Антифриз Умаг поиск ${RUN}`)
  await aziz.waitForTimeout(1_000)
  await expect(aziz.getByTestId('revision-umag')).toHaveCount(0)
  await cancelDraft(page.request, cat.id)
})
