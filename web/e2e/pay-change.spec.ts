// «Изменить оплату» чека (SPEC-37 §2, ADR-132): окно распределяет итог по способам, сервер
// показывает «Что изменится» пробным проведением, «Сохранить» открывает новый чек со ссылкой на
// исходный. Товары и цены не меняются.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e37a' }
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

test('изменить оплату: распределение, «Что изменится», новый чек', async ({ page }) => {
  await login(page)
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Кат оплаты ${RUN}`, kind: 'other' })
  const item = await apiPost<{ id: string }>(page.request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Фильтр оплаты ${RUN}`,
    sale_price_tyiyn: 50_000,
  })
  await apiPost(page.request, '/receipts', {
    op_id: crypto.randomUUID(),
    paid_from: await ownerSafe(page.request),
    lines: [{ product_id: item.id, qty: 5, cost_tyiyn: 100_000 }],
  })
  const cashier = await apiPost<{ id: string }>(page.request, '/employees', { full_name: `Кассир оплаты ${RUN}`, is_cashier: true, pin: '1234' })
  const sale = await apiPost<{ id: string; number: number }>(page.request, '/sales', {
    op_id: crypto.randomUUID(),
    sale_type: 'takeaway',
    cashier_id: cashier.id,
    lines: [{ kind: 'piece', product_id: item.id, qty: 1, unit_price_tyiyn: 50_000 }],
    payments: [{ method: 'card', amount_tyiyn: 50_000 }],
  })

  await page.goto(`/sales/${sale.id}`)
  await page.getByTestId('pay-change-button').click()
  const box = page.getByTestId('pay-change')
  await expect(box).toContainText('Оплата сейчас')
  await expect(page.getByTestId('pay-card')).toHaveValue('500')

  // Перекладываем: 300 наличными, 200 картой; пока не сошлось — «Осталось распределить».
  await page.getByTestId('pay-card').fill('')
  await page.getByTestId('pay-cash').fill('300')
  await expect(page.getByTestId('pay-rest')).toContainText(money('200 с'))
  await expect(page.getByTestId('pay-save')).toBeDisabled()
  await page.getByTestId('pay-card').fill('200')
  await expect(page.getByTestId('pay-rest')).toHaveCount(0)
  // «Что изменится» — от сервера, пробным проведением.
  await expect(page.getByTestId('pay-effect')).toContainText('Что изменится')
  // Причина обязательна.
  await expect(page.getByTestId('pay-save')).toBeDisabled()
  await page.getByTestId('pay-comment').fill('часть оплатили наличными')
  await page.getByTestId('pay-save').click()

  // Новый чек: «Изменение оплаты № N», итог тот же, оплата — новая.
  await expect(page).toHaveURL(/\/sales\/[0-9a-f-]+$/)
  await expect(page).not.toHaveURL(new RegExp(sale.id))
  await expect(page.getByTestId('correction-of')).toContainText(`Изменение оплаты № ${sale.number}`)
  await expect(page.getByText(money('Итого: 500 с'))).toBeVisible()
  // Исходный: «Изменена оплата → № M».
  await page.goto(`/sales/${sale.id}`)
  await expect(page.getByTestId('corrected-by')).toContainText('Изменена оплата →')
})

test('режим кассы не затронут: в кассе кнопки «Изменить оплату» нет', async ({ page }) => {
  await login(page)
  await page.goto('/')
  await expect(page.getByTestId('pay-change-button')).toHaveCount(0)
})
