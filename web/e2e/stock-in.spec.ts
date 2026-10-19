// Оприходование — вариант прихода (SPEC-29 §8, ADR-086): «Приход» → «Новый приход» →
// «Оприходование (без поставщика)», две строки одним документом; в списке приходов с отметкой,
// отбор по виду; сторно всего документа.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2f6' }
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

test('«Приход» → «Оприходование»: две строки, отметка в списке, сторно', async ({ page }) => {
  await login(page)
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Оприходование ${RUN}`, kind: 'other' })
  const goods = [
    { name: `Хомут ОП ${RUN}`, code: `53${RUN}707` },
    { name: `Шайба ОП ${RUN}`, code: `54${RUN}808` },
  ]
  const ids: string[] = []
  for (const g of goods) {
    const p = await apiPost<{ id: string }>(page.request, '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name: g.name, barcodes: [g.code], sale_price_tyiyn: 10_000 })
    ids.push(p.id)
  }

  await page.getByRole('link', { name: 'Приход', exact: true }).click()
  await page.getByRole('button', { name: 'Новый приход' }).first().click()
  await page.getByRole('radio', { name: 'Оприходование (без поставщика)' }).click()
  // Без поставщика и без оплаты: этих полей нет.
  await expect(page.locator('#receipt-supplier')).toHaveCount(0)
  await expect(page.getByRole('radiogroup', { name: 'Расчёт с поставщиком' })).toHaveCount(0)
  const picker = page.locator('#receipt-picker')
  for (const g of goods) {
    await picker.fill(g.code)
    await picker.press('Enter')
    await expect(page.getByText(g.name).first()).toBeVisible()
  }
  await page.locator('#line-1-count').fill('2')
  await page.locator('#line-1-price').fill('30')
  await page.locator('#line-2-count').fill('5')
  await page.locator('#line-2-price').fill('4')
  await expect(page.getByTestId('receipt-total')).toHaveText(money('80 с'))
  const submit = page.getByRole('button', { name: 'Провести оприходование' })
  await expect(submit).toBeDisabled()
  await page.locator('#receipt-comment').fill(`привёз владелец ${RUN}`)
  await submit.click()

  await expect(page.getByRole('heading', { name: /^Оприходование № \d+$/ })).toBeVisible()
  await expect(page.getByTestId('stock-in-line')).toHaveCount(2)
  await expect(page.getByTestId('stock-in-total')).toHaveText(money('Итого: 80 с'))
  for (const [i, qty] of [2, 5].entries()) {
    expect((await apiGet<{ stock_qty: number }>(page.request, `/products/${ids[i]}`)).stock_qty).toBe(qty)
  }

  // В списке приходов — с отметкой «Оприходование»; отбор «Оприходования» и «Все» его показывают.
  await page.getByRole('button', { name: 'К списку' }).click()
  await expect(page.locator('#receipts-kind')).toHaveValue('stock_in')
  const row = page.getByTestId('receipt-row').filter({ hasText: `привёз владелец ${RUN}` })
  await expect(row).toContainText('Оприходование')
  await expect(row).toContainText(goods[0].name)
  await expect(row).toContainText(goods[1].name)
  await expect(row).toContainText(money('80 с'))
  await page.locator('#receipts-kind').selectOption('all')
  await expect(row).toBeVisible()
  await page.locator('#receipts-kind').selectOption('receipt')
  await expect(row).toHaveCount(0)
  await page.locator('#receipts-kind').selectOption('stock_in')

  // Сторно — всего документа, с причиной.
  await row.click()
  await page.getByRole('button', { name: 'Сторно' }).click()
  await page.getByLabel('Причина').fill('ошиблись документом')
  await page.getByRole('button', { name: 'Сторнировать' }).click()
  // Окно сторно закрылось — открыт документ сторно (у окна тот же заголовок).
  await expect(page.getByRole('dialog')).toHaveCount(0)
  await expect(page.getByRole('heading', { name: /^Сторно оприходования № \d+$/ })).toBeVisible()
  for (const id of ids) {
    expect((await apiGet<{ stock_qty: number }>(page.request, `/products/${id}`)).stock_qty).toBe(0)
  }
  await page.getByRole('button', { name: 'К списку' }).click()
  await expect(row).toContainText('сторнировано')
  expect(await apiGet<unknown[]>(page.request, '/stock/verify')).toEqual([])
})
