// Один товар лежит в нескольких местах, его сканируют разные люди с разных устройств в одной
// ревизии — итог складывается и виден всем (задача 003, Н14).
import { expect, test, type APIRequestContext, type Browser, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const RUN = Date.now().toString().slice(-8)
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2f1' }

/** Отдельное устройство: свой контекст браузера, свой вход. */
async function device(browser: Browser): Promise<Page> {
  const page = await (await browser.newContext()).newPage()
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
  return page
}

async function apiPost<T>(request: APIRequestContext, path: string, data: unknown): Promise<T> {
  const res = await request.post(`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

async function openRevision(page: Page, category: string) {
  await page.goto('/revision')
  await page.getByRole('row', { name: new RegExp(category) }).first().click()
  await expect(page.getByRole('heading', { name: new RegExp(`Ревизия № \\d+ · ${category}`) })).toBeVisible()
}

async function scan(page: Page, code: string) {
  const scanner = page.getByPlaceholder('Сканируйте штрихкод')
  await scanner.fill(code)
  await scanner.press('Enter')
}

test('ревизия: два устройства сканируют один товар — у обоих итог 2', async ({ browser }) => {
  const a = await device(browser)
  const b = await device(browser)
  const category = `Кат два устройства ${RUN}`
  const code = `27${RUN}401`
  const cat = await apiPost<{ id: string }>(a.request, '/categories', { name: category, kind: 'other' })
  const p = await apiPost<{ id: string }>(a.request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Свеча два устройства ${RUN}`,
    barcodes: [code],
    sale_price_tyiyn: 30_000,
  })
  await apiPost(a.request, '/revisions', { category_id: cat.id })
  await openRevision(a, category)
  await openRevision(b, category)

  // Оба открыли строку с пустым пересчётом; каждый сканирует по разу.
  await Promise.all([scan(a, code), scan(b, code)])
  // Свой скан виден сразу, чужой — при перечитывании (раз в 5 с): у обоих 2, а не 1.
  for (const page of [a, b]) {
    await expect(page.locator(`[data-count="${p.id}"]`)).toHaveValue('2', { timeout: 15_000 })
    await expect(page.getByTestId('counted-by').first()).toContainText('последним считал')
  }

  // Ещё скан на первом — второй увидит 3, не трогая своё поле.
  await scan(a, code)
  await expect(a.locator(`[data-count="${p.id}"]`)).toHaveValue('3')
  await expect(b.locator(`[data-count="${p.id}"]`)).toHaveValue('3', { timeout: 15_000 })

  // Ручной ввод — итог строки («set»): второй вписал 5 — у обоих 5.
  const field = b.locator(`[data-count="${p.id}"]`)
  await field.fill('5')
  await field.press('Enter')
  await expect(field).toHaveValue('5')
  await expect(a.locator(`[data-count="${p.id}"]`)).toHaveValue('5', { timeout: 15_000 })
})

test('ревизия: незнакомый код завёл другой — второй прибавляет к его товару', async ({ browser }) => {
  const a = await device(browser)
  const b = await device(browser)
  const category = `Кат новый код ${RUN}`
  const code = `27${RUN}402`
  const cat = await apiPost<{ id: string }>(a.request, '/categories', { name: category, kind: 'other' })
  await apiPost(a.request, '/revisions', { category_id: cat.id })
  await openRevision(a, category)
  await openRevision(b, category)

  // Оба сканируют код, которого нет в программе: у обоих — окно новой карточки.
  for (const page of [a, b]) {
    await scan(page, code)
    await expect(page.getByRole('heading', { name: 'Товар, которого нет в программе' })).toBeVisible()
    await page.locator('#revnew-name').fill(`Щётка новый код ${RUN}`)
    await page.locator('#revnew-price').fill('100')
    await page.locator('#revnew-count').fill('1')
  }
  // Первый заводит товар; второй сохраняет следом — товар уже есть, его найденное прибавляется.
  await a.getByRole('button', { name: /Завести/ }).click()
  await expect(a.getByText(/Заведён и посчитан/)).toBeVisible()
  await b.getByRole('button', { name: /Завести/ }).click()
  await expect(b.getByText('Этот товар только что завёл другой — добавили к нему')).toBeVisible()

  const rows = await a.request.get(`/api/v1/products?q=${encodeURIComponent(`Щётка новый код ${RUN}`)}`)
  const found = (await rows.json()) as { id: string }[]
  expect(found).toHaveLength(1)
  for (const page of [a, b]) {
    await expect(page.locator(`[data-count="${found[0].id}"]`)).toHaveValue('2', { timeout: 15_000 })
  }
})
