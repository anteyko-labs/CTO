// Приём аккумуляторов на кассе виден в «Приходе» (задача 025): строка с весом и ценой за кг,
// отбор «Приём аккумуляторов», окно приёма; отменённый приём — с отметкой «отменён».
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ensureShift } from './cashier'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e025' }
const RUN = Date.now().toString().slice(-8)

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function post<T>(request: APIRequestContext, path: string, data: unknown): Promise<T> {
  const res = await request.post(`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

test('приём аккумуляторов с кассы — строкой в «Приходе»', async ({ page }) => {
  await login(page)
  await ensureShift(page.request)
  await post(page.request, '/cash/movements', { op_id: crypto.randomUUID(), kind: 'cash_in', amount_tyiyn: 500_000, comment: 'размен' })
  await post(page.request, '/batteries/intake', {
    op_id: crypto.randomUUID(),
    grams: 12_500,
    price_per_kg_tyiyn: 10_000,
    comment: `Нексия ${RUN}`,
  })
  const wrong = await post<{ id: string }>(page.request, '/batteries/intake', {
    op_id: crypto.randomUUID(),
    grams: 2_000,
    price_per_kg_tyiyn: 8_000,
    comment: `Ошибка ${RUN}`,
  })
  await post(page.request, `/batteries/intakes/${wrong.id}/reverse`, { op_id: crypto.randomUUID(), comment: 'ошиблись весом' })

  await page.goto('/receipts')
  await page.locator('#receipts-kind').selectOption('battery')
  await page.locator('#receipts-search').fill(RUN)
  const rows = page.getByTestId('receipt-row')
  await expect(rows).toHaveCount(2)
  const good = rows.filter({ hasText: `Нексия ${RUN}` })
  await expect(good).toContainText(/12,5\sкг по 100\sс\/кг/)
  await expect(good).toContainText('Приём аккумуляторов')
  await expect(good).toContainText(/1\s250\sс/)
  await expect(rows.filter({ hasText: `Ошибка ${RUN}` })).toContainText('отменён')

  // Окно приёма: касса, кто принял; у отменённого — кто и почему отменил.
  await good.click()
  const card = page.getByTestId('battery-intake-card')
  await expect(card).toContainText('12,5')
  await expect(card).toContainText(`Нексия ${RUN}`)
  await page.keyboard.press('Escape')
  await expect(card).toBeHidden()
  await rows.filter({ hasText: `Ошибка ${RUN}` }).click()
  await expect(page.getByTestId('battery-intake-card')).toContainText('ошиблись весом')
  await page.keyboard.press('Escape')

  // «Все» показывает приём вместе с накладными; «От поставщиков» — без него.
  await page.locator('#receipts-kind').selectOption('all')
  await expect(rows).toHaveCount(2)
  await page.locator('#receipts-kind').selectOption('receipt')
  await expect(page.getByText('Ничего не нашлось')).toBeVisible()
})
