// Ревизию начинает только владелец (задача 019, ADR-186): администратор не видит «Начать ревизию»,
// в «Сканере товаров» пересчёт ему недоступен; владелец начал — администратор присоединяется,
// считает сканом, но не отменяет.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e019' }
const RUN = Date.now().toString().slice(-8)

async function login(page: Page, user: string, password: string) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(user)
  await page.getByLabel('Пароль').fill(password)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'get' | 'post', path: string, data?: unknown): Promise<T> {
  const res = await request[method](`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

/** Открытые черновики мешают проверить «ждать владельца» — отменяет владелец. */
async function cancelDrafts(request: APIRequestContext) {
  const list = await api<{ id: string; status: string }[]>(request, 'get', '/revisions')
  for (const r of list.filter((x) => x.status === 'draft')) await api(request, 'post', `/revisions/${r.id}/cancel`, {})
}

test('ревизию начинает владелец, администратор присоединяется и считает', async ({ page, browser }) => {
  await login(page, OWNER, OWNER_PASSWORD)
  await cancelDrafts(page.request)
  const code = `219${RUN}`
  const cat = await api<{ id: string }>(page.request, 'post', '/categories', { name: `Кат владелец начинает ${RUN}`, kind: 'other' })
  const p = await api<{ id: string }>(page.request, 'post', '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Свеча владелец начинает ${RUN}`,
    barcodes: [code],
    sale_price_tyiyn: 30_000,
  })
  const login_ = `revstart${RUN}`
  const password = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  await api(page.request, 'post', '/users', { login: login_, password, full_name: `Админ ревизии ${RUN}`, role: 'admin' })
  const ctx = await browser.newContext()
  const a = await ctx.newPage()
  await login(a, login_, password)

  // Черновика нет: начать нельзя ни кнопкой, ни запросом (I12).
  await a.goto('/revision')
  await expect(a.getByTestId('revision-wait-owner')).toBeVisible()
  await expect(a.getByRole('button', { name: /Начать/ })).toHaveCount(0)
  const denied = await a.request.post('/api/v1/revisions', { data: { category_id: null }, headers: DEVICE })
  expect(denied.status()).toBe(403)
  expect(((await denied.json()) as { error: { message: string } }).error.message).toBe('Ревизию начинает владелец')
  // «Сканер товаров»: пересчёт недоступен.
  await a.goto('/revision?tab=scan')
  await expect(a.getByTestId('intake-wait-owner')).toBeVisible()
  await expect(a.getByRole('checkbox', { name: /Пересчёт \(ревизия\)/ })).toBeDisabled()

  // Владелец начал «весь склад» — администратор присоединяется и сканирует.
  const rev = await api<{ head: { id: string; number: number } }>(page.request, 'post', '/revisions', { category_id: null })
  await a.goto('/revision')
  await a.getByRole('button', { name: `Присоединиться к ревизии № ${rev.head.number} · весь склад` }).click()
  await expect(a.getByRole('heading', { name: new RegExp(`Ревизия № ${rev.head.number} · весь склад`) })).toBeVisible()
  await expect(a.getByRole('button', { name: 'Отменить ревизию' })).toHaveCount(0)
  const scanner = a.getByPlaceholder('Сканируйте штрихкод')
  await scanner.fill(code)
  await scanner.press('Enter')
  await expect
    .poll(async () => (await api<{ counted_qty: number | null }>(page.request, 'get', `/revisions/${rev.head.id}/lines/${p.id}`)).counted_qty, {
      timeout: 15_000,
    })
    .toBe(1)
  const cancel = await a.request.post(`/api/v1/revisions/${rev.head.id}/cancel`, { data: {}, headers: DEVICE })
  expect(cancel.status()).toBe(403)
  // В «Сканере товаров» пересчёт снова доступен.
  await a.goto('/revision?tab=scan')
  await expect(a.getByRole('checkbox', { name: /Пересчёт \(ревизия\)/ })).toBeEnabled()
  await ctx.close()
  await api(page.request, 'post', `/revisions/${rev.head.id}/cancel`, {})
})
