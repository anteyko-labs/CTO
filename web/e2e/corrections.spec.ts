// «Исправить» у документов владельца (SPEC-29 §24): чек и приход исправляются в той же форме,
// что создаются; исходный сторнируется, верный проводится, оба ссылаются друг на друга.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2fc' }
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

async function product(request: APIRequestContext, name: string, price: number) {
  const cat = await apiPost<{ id: string }>(request, '/categories', { name: `Кат ${name}`, kind: 'other' })
  return apiPost<{ id: string }>(request, '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name, sale_price_tyiyn: price })
}

test('исправить чек: количество в той же кассе, новый чек со ссылкой на исходный', async ({ page }) => {
  await login(page)
  const item = await product(page.request, `Фильтр исправления ${RUN}`, 50_000)
  await apiPost(page.request, '/receipts', {
    op_id: crypto.randomUUID(),
    paid_from: await ownerSafe(page.request),
    lines: [{ product_id: item.id, qty: 10, cost_tyiyn: 200_000 }],
  })
  const cashier = await apiPost<{ id: string }>(page.request, '/employees', { full_name: `Кассир исправления ${RUN}`, is_cashier: true, pin: '1234' })
  // Картой: возврат на карту смены не требует — проверка не зависит от открытой смены.
  const sale = await apiPost<{ id: string; number: number }>(page.request, '/sales', {
    op_id: crypto.randomUUID(),
    sale_type: 'takeaway',
    cashier_id: cashier.id,
    lines: [{ kind: 'piece', product_id: item.id, qty: 1, unit_price_tyiyn: 50_000 }],
    payments: [{ method: 'card', amount_tyiyn: 50_000 }],
  })

  await page.goto(`/sales/${sale.id}`)
  await page.getByTestId('fix-button').click()
  await expect(page).toHaveURL(new RegExp(`/sales/${sale.id}/correct$`))
  await expect(page.getByTestId('correction-banner')).toContainText(`№ ${sale.number}`)
  // Касса заполнена исходным чеком: строка, оплата картой; кассир — исходного чека.
  const qty = page.getByRole('textbox', { name: 'Кол-во' })
  await expect(qty).toHaveValue('1')
  await expect(page.locator('#cashier-select')).not.toHaveValue('')
  await qty.fill('2')
  const save = page.getByRole('button', { name: 'Сохранить исправление' })
  await expect(save).toBeDisabled()
  await page.locator('#correction-comment').fill('кассир пробил одну штуку вместо двух')
  await save.click()

  // Новый чек: 1 000 с, «исправление № N» и что поменялось.
  await expect(page).toHaveURL(/\/sales\/[0-9a-f-]+$/)
  await expect(page).not.toHaveURL(new RegExp(sale.id))
  const note = page.getByTestId('correction-of')
  await expect(note).toContainText(`Исправление № ${sale.number}`)
  await expect(note.getByTestId('diff-summary')).toContainText('количество')
  await expect(page.getByText(money('Итого: 1 000 с'))).toBeVisible()
  // Исходный: «исправлен → № M», второй раз не исправляется.
  await page.goto(`/sales/${sale.id}`)
  await expect(page.getByTestId('corrected-by')).toContainText('Исправлен →')
  await expect(page.getByTestId('fix-button')).toHaveCount(0)
})

test('исправить приход: количество и номер накладной, долг поставщику — по исправленной', async ({ page }) => {
  await login(page)
  const item = await product(page.request, `Свеча исправления ${RUN}`, 30_000)
  const supplier = await apiPost<{ id: string; name: string }>(page.request, '/suppliers', { name: `Поставщик исправления ${RUN}` })
  const receipt = await apiPost<{ id: string; number: number }>(page.request, '/receipts', {
    op_id: crypto.randomUUID(),
    supplier_id: supplier.id,
    payment: 'debt',
    supplier_doc: `Н-${RUN}`,
    lines: [{ product_id: item.id, qty: 3, cost_tyiyn: 30_000 }],
  })

  await page.goto(`/receipts/${receipt.id}`)
  await page.getByTestId('fix-button').click()
  await expect(page).toHaveURL(new RegExp(`/receipts/${receipt.id}/correct$`))
  await expect(page.getByRole('heading', { name: `Исправление прихода № ${receipt.number}` })).toBeVisible()
  await expect(page.locator('#receipt-supplier')).toHaveValue(supplier.id)
  await expect(page.locator('#line-1-count')).toHaveValue('3')
  await expect(page.locator('#line-1-price')).toHaveValue('100')
  await page.locator('#line-1-count').fill('5')
  await page.locator('#correction-comment').fill('пришло пять, а не три')
  await page.getByRole('button', { name: 'Сохранить исправление' }).click()

  await expect(page).toHaveURL(/\/receipts\/[0-9a-f-]+$/)
  await expect(page).not.toHaveURL(new RegExp(receipt.id))
  await expect(page.getByTestId('correction-of')).toContainText(`Исправление № ${receipt.number}`)
  await expect(page.getByTestId('correction-of').getByTestId('diff-summary')).toContainText('5 шт')
  // Долг поставщику — по исправленной накладной: 500 с, а не 300 + 500.
  const res = await page.request.get(`/api/v1/parties/${supplier.id}`)
  expect(((await res.json()) as { balance_tyiyn: number }).balance_tyiyn).toBe(-50_000)
  await page.goto(`/receipts/${receipt.id}`)
  await expect(page.getByTestId('corrected-by')).toContainText('Исправлен →')
})
