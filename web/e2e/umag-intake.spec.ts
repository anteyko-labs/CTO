// Разбор остатков из Умага (задача 042): загрузить выгрузку с «Остатком», «Новый товар»,
// «Уже есть в Автодом» → «Это он», провести ревизию — остатки встают.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { xlsxBytes } from '../src/lib/xlsxWrite'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e042' }
const RUN = Date.now().toString().slice(-8)

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' }).first()).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'get' | 'post', path: string, data?: unknown): Promise<T> {
  const res = await request[method](`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

test('разбор из Умага: новый товар, «Это он», проведение ревизии', async ({ page }) => {
  await login(page)
  const req = page.request
  for (const r of (await api<{ id: string; status: string }[]>(req, 'get', '/revisions')).filter((x) => x.status === 'draft'))
    await api(req, 'post', `/revisions/${r.id}/cancel`, {})
  const catName = `Разбор ${RUN}`
  await api(req, 'post', '/categories', { name: catName, kind: 'other' })
  const cats = await api<{ id: string; name: string }[]>(req, 'get', '/categories')
  const cat = cats.find((c) => c.name === catName)!
  const oldCode = `2042${RUN}1`
  const existing = await api<{ id: string }>(req, 'post', '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Свеча старая ${RUN}`,
    barcodes: [oldCode],
    sale_price_tyiyn: 30_000,
  })
  const newName = `Щётка новая ${RUN}`
  const file = xlsxBytes([
    {
      name: 'Товары',
      columns: ['Название товара', 'Штрихкод', 'Закупочная цена', 'Прод. цена', 'Ед. изм.', 'Категория', 'Остаток'].map((label) => ({ label })),
      rows: [
        [newName, `2042${RUN}2`, '100', '250', 'шт', catName, '5'],
        [`СВЕЧА ${RUN} из Умага`, oldCode, '150', '300', 'шт', catName, '3'],
      ],
      filter: false,
    },
  ])

  await page.goto('/stock/umag-intake')
  await page.getByTestId('intake-upload').setInputFiles({
    name: 'umag.xlsx',
    mimeType: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
    buffer: Buffer.from(file),
  })
  await expect(page.getByText('Загружено позиций: 2')).toBeVisible()
  await page.getByPlaceholder('Поиск: название, штрихкод, артикул').fill(RUN)
  const rowNew = page.getByTestId('intake-row').filter({ hasText: newName })
  await expect(rowNew).toContainText('5')

  // «Новый товар»: карточка заполнена из строки — только сохранить.
  await rowNew.getByRole('button', { name: 'Новый товар' }).click()
  const dlg = page.getByRole('dialog')
  await expect(dlg.getByLabel('Название')).toHaveValue(newName)
  await dlg.getByRole('button', { name: 'Сохранить' }).click()
  await expect(rowNew).toHaveCount(0)

  // «Уже есть в Автодом» → «Это он».
  const rowOld = page.getByTestId('intake-row').filter({ hasText: `СВЕЧА ${RUN}` })
  await rowOld.getByRole('button', { name: 'Уже есть в Автодом' }).click()
  const sim = page.getByTestId('intake-similar')
  await expect(sim).toContainText(`Свеча старая ${RUN}`)
  await sim.getByRole('button', { name: 'Это он' }).first().click()
  await expect(rowOld).toHaveCount(0)

  await page.getByRole('button', { name: 'Привязаны' }).click()
  await expect(page.getByTestId('intake-row').filter({ hasText: `СВЕЧА ${RUN}` })).toContainText(`Свеча старая ${RUN}`)

  // Проведение ревизии — остатки встают.
  const draft = (await api<{ id: string; status: string; category_id: string | null }[]>(req, 'get', '/revisions')).find((r) => r.status === 'draft')!
  expect(draft.category_id).toBeNull()
  await api(req, 'post', `/revisions/${draft.id}/post`, { op_id: crypto.randomUUID(), force: true, comment: 'Остатки из Умага' })
  const after = await api<{ stock_qty: number }>(req, 'get', `/products/${existing.id}`)
  expect(after.stock_qty).toBe(3)
  const made = await api<{ name: string; stock_qty: number }[]>(req, 'get', `/products?q=${encodeURIComponent(newName)}`)
  expect(made.find((p) => p.name === newName)?.stock_qty).toBe(5)
})
