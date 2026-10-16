// Страницы длинных списков (SPEC-28 §2): «⏮ ◀ 1 2 … ▶ ⏭  1–25 / N  по 25/50/100», смена отбора
// возвращает на первую страницу, итоги товаров — по всему отбору, а не по странице.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2e9' }
const RUN = Date.now().toString().slice(-8)
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

const two = (i: number) => String(i).padStart(2, '0')

test('клиенты по страницам: 25 на странице, вторая страница, отбор — снова первая', async ({ page }) => {
  await login(page)
  const prefix = `Страничный ${RUN}`
  for (let i = 1; i <= 30; i++) {
    await apiPost(page.request, '/parties', { role: 'customer', kind: 'person', name: `${prefix} ${two(i)}` })
  }

  await page.goto('/clients')
  await page.getByLabel('Поиск').fill(prefix)
  const pager = page.getByRole('navigation', { name: 'Страницы клиентов' })
  const range = pager.getByTestId('pager-range')
  // По умолчанию 50 на странице: все 30 на одной.
  await expect(range).toHaveText('1–30 / 30')
  await pager.getByLabel('Строк на странице').selectOption('25')
  await expect(range).toHaveText('1–25 / 30')
  await expect(page.getByRole('row', { name: new RegExp(`${prefix} 25`) })).toBeVisible()
  await expect(page.getByRole('row', { name: new RegExp(`${prefix} 26`) })).toHaveCount(0)
  await expect(pager.getByRole('button', { name: 'Предыдущая страница' })).toBeDisabled()

  // Вторая страница: остальные пять.
  await pager.getByRole('button', { name: '2', exact: true }).click()
  await expect(range).toHaveText('26–30 / 30')
  await expect(page.getByRole('row', { name: new RegExp(`${prefix} 26`) })).toBeVisible()
  await expect(page.getByRole('row', { name: new RegExp(`${prefix} 30`) })).toBeVisible()
  await expect(page.getByRole('row', { name: new RegExp(`${prefix} 01`) })).toHaveCount(0)
  await expect(pager.getByRole('button', { name: 'Следующая страница' })).toBeDisabled()
  await expect(pager.getByRole('button', { name: 'Последняя страница' })).toBeDisabled()

  // Назад и снова в конец — кнопками «первая» и «последняя».
  await pager.getByRole('button', { name: 'Первая страница' }).click()
  await expect(range).toHaveText('1–25 / 30')
  await pager.getByRole('button', { name: 'Последняя страница' }).click()
  await expect(range).toHaveText('26–30 / 30')

  // Смена отбора — снова первая страница; размер страницы запомнился.
  await page.getByLabel('Вид').selectOption({ label: 'Физлица' })
  await expect(range).toHaveText('1–25 / 30')
  await expect(page.getByRole('row', { name: new RegExp(`${prefix} 01`) })).toBeVisible()
  await page.reload()
  await page.getByLabel('Поиск').fill(prefix)
  await expect(page.getByRole('navigation', { name: 'Страницы клиентов' }).getByTestId('pager-range')).toHaveText('1–25 / 30')
})

test('товары по страницам: страницу режет сервер, итоги — по всему отбору', async ({ page }) => {
  await login(page)
  const category = `Страницы ${RUN}`
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: category, kind: 'other' })
  const lines: { product_id: string; qty: number; cost_tyiyn: number }[] = []
  for (let i = 1; i <= 27; i++) {
    // Каждый товар: 1 шт, закупка 100 с, продажа 200 с.
    const p = await apiPost<{ id: string }>(page.request, '/products', {
      op_id: crypto.randomUUID(),
      category_id: cat.id,
      name: `Листаемый ${RUN} ${two(i)}`,
      sale_price_tyiyn: 20_000,
    })
    lines.push({ product_id: p.id, qty: 1, cost_tyiyn: 10_000 })
  }
  await apiPost(page.request, '/receipts', { op_id: crypto.randomUUID(), paid_from: 'outside', lines })

  await page.goto('/products')
  await page.getByLabel('Категория').selectOption({ label: category })
  const table = page.getByRole('table', { name: 'Список товаров' })
  const pager = page.getByRole('navigation', { name: 'Страницы товаров' })
  const range = pager.getByTestId('pager-range')
  const totals = page.getByTestId('product-totals')
  await expect(range).toHaveText('1–27 / 27')
  await pager.getByLabel('Строк на странице').selectOption('25')
  await expect(range).toHaveText('1–25 / 27')
  // Строка заголовка + 25 товаров.
  await expect(table.getByRole('row')).toHaveCount(26)
  await expect(totals).toContainText('Итого по отбору: 27 товаров')
  await expect(totals).toContainText(money('по закупке 2 700 с'))
  await expect(totals).toContainText(money('по продаже 5 400 с'))

  await pager.getByRole('button', { name: 'Следующая страница' }).click()
  await expect(range).toHaveText('26–27 / 27')
  await expect(table.getByRole('row')).toHaveCount(3)
  await expect(table.getByRole('row', { name: new RegExp(`Листаемый ${RUN} 27`) })).toBeVisible()
  // Итоги не зависят от страницы.
  await expect(totals).toContainText('Итого по отбору: 27 товаров')
  await expect(totals).toContainText(money('по продаже 5 400 с'))

  // Сортировка — снова первая страница: по названию в обратном порядке сверху 27-й.
  await table.getByRole('button', { name: 'Название' }).click()
  await expect(range).toHaveText('1–25 / 27')
  await expect(table.getByRole('row').nth(1)).toContainText(`Листаемый ${RUN} 27`)
  await expect(totals).toContainText('Итого по отбору: 27 товаров')
})
