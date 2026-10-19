// Удобный доступ и подписи оплат (SPEC-29 §25): «Оприходование» и «Возврат поставщику» в меню
// «Склад», «Внести / Изъять» в меню «Деньги», подписи кнопок оплаты на кассе — куда идут деньги.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2f5' }
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

async function apiGet<T>(request: APIRequestContext, path: string): Promise<T> {
  const res = await request.get(`/api/v1${path}`)
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

async function product(request: APIRequestContext, category: string, name: string, code: string, price: number) {
  return apiPost<{ id: string }>(request, '/products', { op_id: crypto.randomUUID(), category_id: category, name, barcodes: [code], sale_price_tyiyn: price })
}

test('меню «Склад» → «Оприходование» → «+ Оприходовать»: документ в списке', async ({ page }) => {
  await login(page)
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Меню ${RUN}`, kind: 'other' })
  const name = `Шайба ${RUN}`
  const code = `51${RUN}505`
  const p = await product(page.request, cat.id, name, code, 5_000)
  await page.getByRole('link', { name: 'Оприходование' }).click()
  await expect(page.getByRole('heading', { name: 'Оприходование' })).toBeVisible()
  await page.getByRole('button', { name: 'Оприходовать' }).first().click()
  const picker = page.getByPlaceholder(/товар, который оприходуем/)
  await picker.fill(code)
  await picker.press('Enter')
  const dialog = page.getByRole('dialog', { name: `Оприходовать: ${name}` })
  await dialog.locator('#stockin-qty').fill('2')
  await dialog.locator('#stockin-price').fill('30')
  await dialog.locator('#stockin-comment').fill(`из меню ${RUN}`)
  await dialog.getByRole('button', { name: 'Оприходовать' }).click()
  await expect(page.getByText(/Оприходовано № \d+: 2 шт/)).toBeVisible()
  await dialog.getByRole('button', { name: 'Закрыть' }).first().click()
  const row = page.getByTestId('stock-in-row').filter({ hasText: name })
  await expect(row).toContainText(`из меню ${RUN}`)
  await expect(row).toContainText(money('60 с'))
  expect((await apiGet<{ stock_qty: number }>(page.request, `/products/${p.id}`)).stock_qty).toBe(2)
})

test('меню «Склад» → «Возврат поставщику» → «+»: возврат в общем списке', async ({ page }) => {
  await login(page)
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Меню возврат ${RUN}`, kind: 'other' })
  const supplier = await apiPost<{ id: string }>(page.request, '/suppliers', { name: `Из меню ${RUN}` })
  const code = `52${RUN}606`
  const p = await product(page.request, cat.id, `Скоба ${RUN}`, code, 20_000)
  await apiPost(page.request, '/receipts', {
    op_id: crypto.randomUUID(),
    supplier_id: supplier.id,
    payment: 'debt',
    lines: [{ product_id: p.id, qty: 5, cost_tyiyn: 50_000 }],
  })
  await page.getByRole('link', { name: 'Возврат поставщику' }).click()
  await expect(page.getByRole('heading', { name: 'Возврат поставщику' })).toBeVisible()
  await page.getByRole('button', { name: 'Возврат поставщику' }).click()
  await page.locator('#return-supplier').selectOption(supplier.id)
  await page.getByRole('button', { name: 'Дальше' }).click()
  const dialog = page.getByRole('dialog', { name: new RegExp(`Возврат поставщику: Из меню ${RUN}`) })
  const picker = dialog.getByPlaceholder(/товар, который возвращаем/)
  await picker.fill(code)
  await picker.press('Enter')
  await dialog.locator('#return-qty-1').fill('2')
  await expect(dialog.locator('#return-amount-1')).toHaveValue('200')
  await dialog.getByRole('button', { name: 'Провести возврат' }).click()
  await expect(dialog).toBeHidden()
  const row = page.getByTestId('supplier-return-row').filter({ hasText: `Скоба ${RUN}` })
  await expect(row).toContainText(`Из меню ${RUN}`)
  await expect(row).toContainText('в счёт долга')
  await expect(row).toContainText(money('200 с'))
  expect((await apiGet<{ stock_qty: number }>(page.request, `/products/${p.id}`)).stock_qty).toBe(3)
})

test('меню «Деньги» → «Внести / Изъять»: счёт выбирают в окне', async ({ page }) => {
  await login(page)
  const before = await apiGet<{ id: string; name: string; balance_tyiyn: number }[]>(page.request, '/cash/accounts')
  const safe = before.find((a) => a.name === 'Сейф магазина')!
  await page.getByRole('link', { name: 'Внести / Изъять' }).click()
  const dialog = page.getByTestId('account-money')
  await expect(dialog).toBeVisible()
  await dialog.locator('#account-money-from').selectOption(safe.id)
  await expect(dialog.getByText(/Сейчас на счёте/)).toBeVisible()
  await dialog.locator('#account-money-sum').fill('70')
  await dialog.locator('#account-money-comment').fill(`из меню ${RUN}`)
  await dialog.getByRole('button', { name: 'Внести' }).click()
  await expect(page.getByText(/Внесено 70\sс: Сейф магазина/)).toBeVisible()
  // Кнопки «Внести», «Изъять», «Перевести» — и над списком счетов.
  await expect(page.getByTestId('money-actions').getByRole('button', { name: 'Изъять' })).toBeVisible()
  const after = await apiGet<{ id: string; balance_tyiyn: number }[]>(page.request, '/cash/accounts')
  expect(after.find((a) => a.id === safe.id)!.balance_tyiyn - safe.balance_tyiyn).toBe(7_000)
})

test('касса: «Карта / QR банка» и «QR владельцу» с подписью, куда идут деньги', async ({ page }) => {
  await login(page)
  await page.goto('/')
  const pay = page.locator('[data-pay]')
  await expect(pay.getByRole('button', { name: 'Карта / QR банка', exact: true })).toBeVisible()
  await expect(pay.getByRole('button', { name: 'QR владельцу', exact: true })).toBeVisible()
  await pay.getByRole('button', { name: 'Наличные', exact: true }).click()
  await expect(page.getByTestId('pay-dest')).toHaveText('→ Касса · наличные')
  await pay.getByRole('button', { name: 'Карта / QR банка', exact: true }).click()
  await expect(page.getByTestId('pay-dest')).toHaveText(/^→ .+ · (комиссия [\d,]+ %|без комиссии)$/)
  await pay.getByRole('button', { name: 'QR владельцу', exact: true }).click()
  await expect(page.getByTestId('pay-dest')).toHaveText(/^→ .+ · (комиссия [\d,]+ %|без комиссии)$/)
})
