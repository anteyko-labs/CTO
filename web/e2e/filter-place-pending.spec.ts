// Хвосты задач 041 и 039 на кассе. «Куда поставили» (задача 041 A): масляный фильтр, поставленный как
// фильтр коробки, — в книжке строкой «Фильтр коробки». Машина фирмы ждёт подтверждения (ADR-199):
// пометка у выбора машины, в долг фирме чек не провести — подсказка вместо ответа 409.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ownerSafe } from './safe'
import { choosePerson, openPay, pickMaster, shiftOnTill } from './cashier'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e041' }
const RUN = Date.now().toString().slice(-8)

test.use({ viewport: { width: 1366, height: 768 } })

async function api<T>(request: APIRequestContext, path: string, data?: unknown): Promise<T> {
  const res = data === undefined ? await request.get(`/api/v1${path}`, { headers: DEVICE }) : await request.post(`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.locator('[data-picker]')).toBeVisible()
}

async function scan(page: Page, code: string, lines: number) {
  await page.locator('[data-picker]').focus()
  await page.keyboard.type(code, { delay: 5 })
  await page.keyboard.press('Enter')
  await expect(page.getByTestId('cart').getByRole('listitem')).toHaveCount(lines)
}

test('«Куда поставили»: масляный фильтр как фильтр коробки — в книжке «Фильтр коробки»', async ({ page }) => {
  test.setTimeout(120_000)
  await login(page)
  const request = page.request
  const safe = await ownerSafe(request)
  const oilCat = await api<{ id: string }>(request, '/categories', { name: `Масла места ${RUN}`, kind: 'oil' })
  const filterCat = await api<{ id: string }>(request, '/categories', { name: `Фильтры места ${RUN}`, kind: 'filter' })
  const oilCode = `47${RUN}1`
  const filterCode = `47${RUN}2`
  const oil = await api<{ id: string }>(request, '/products', { op_id: crypto.randomUUID(), category_id: oilCat.id, name: `Масло места ${RUN} 5W-30`, container_ml: 4000, barcodes: [oilCode], sale_price_tyiyn: 300_000 })
  const filterName = `Фильтр места ${RUN}`
  const filter = await api<{ id: string }>(request, '/products', { op_id: crypto.randomUUID(), category_id: filterCat.id, name: filterName, attrs: { filter_type: 'масляный' }, barcodes: [filterCode], sale_price_tyiyn: 60_000 })
  await api(request, '/receipts', {
    op_id: crypto.randomUUID(),
    paid_from: safe,
    lines: [
      { product_id: oil.id, qty: 40_000, cost_tyiyn: 2_000_000 },
      { product_id: filter.id, qty: 10, cost_tyiyn: 300_000 },
    ],
  })
  const service = `Замена масла в моторе места ${RUN}`
  await api(request, '/services', { name: service, price_tyiyn: 20_000, master_fee_tyiyn: 3_000 })
  const master = `Мастер места ${RUN}`
  await api(request, '/employees', { full_name: master, is_master: true })
  const cashier = `Кассир места ${RUN}`
  await api(request, '/employees', { full_name: cashier, is_cashier: true, pin: '1234' })
  const last = `Местов${RUN}`
  const party = await api<{ id: string }>(request, '/parties', { kind: 'person', first_name: 'Азиз', last_name: last, phone: `0703${RUN.slice(-6)}` })
  const plate = `09KG${RUN.slice(-3)}AFP`
  const vehicle = await api<{ id: string }>(request, `/parties/${party.id}/vehicles`, { plate, brand: 'Toyota', model: 'Camry' })
  await page.reload()

  await page.getByRole('button', { name: 'В сервис' }).click()
  const kinds = page.getByRole('dialog', { name: 'Вид замены' })
  await kinds.getByPlaceholder('Найти услугу').fill(service)
  await kinds.getByTestId('service-choices').getByRole('button', { name: new RegExp(service) }).click()
  await kinds.getByRole('button', { name: 'Готово' }).click()
  await expect(kinds).toHaveCount(0)
  await scan(page, oilCode, 2)
  await scan(page, filterCode, 3)
  await shiftOnTill(page, cashier)
  if (await page.locator('#cashier-select').isVisible()) await choosePerson(page, 'cashier-select', cashier)
  await pickMaster(page, master)
  await page.locator('[data-client-input]').fill(plate)
  await page.getByRole('button', { name: new RegExp(last) }).first().click()
  await page.locator('#mileage').fill('217000')

  const place = page.getByTestId('line-place')
  await expect(place).toHaveCount(1)
  await place.selectOption('gearbox_filter')
  await openPay(page)
  await page.getByRole('button', { name: 'Провести чек' }).first().click()
  await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()

  const book = await api<{ records: { lines: { slot: string; text: string }[] }[] }>(request, `/vehicles/${vehicle.id}/oil-book`)
  const lines = book.records[0].lines
  expect(lines.find((l) => l.text.startsWith(filterName))?.slot).toBe('gearbox_filter')
  expect(lines.some((l) => l.slot === 'oil_filter')).toBe(false)
  // На экране книжки клиента — строкой «Фильтр коробки».
  await page.goto(`/clients/${party.id}`)
  await expect(page.getByTestId('book-slots')).toContainText('Фильтр коробки')
})

test('машина ждёт подтверждения фирмы: пометка у машины, в долг — подсказка, чек не проводится', async ({ page }) => {
  test.setTimeout(90_000)
  await login(page)
  const request = page.request
  const safe = await ownerSafe(request)
  const cat = await api<{ id: string }>(request, '/categories', { name: `Прочее фирмы ${RUN}`, kind: 'other' })
  const code = `48${RUN}1`
  const name = `Щётка фирмы ${RUN}`
  const p = await api<{ id: string }>(request, '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name, barcodes: [code], sale_price_tyiyn: 50_000 })
  await api(request, '/receipts', { op_id: crypto.randomUUID(), paid_from: safe, lines: [{ product_id: p.id, qty: 5, cost_tyiyn: 100_000 }] })
  const cashier = `Кассир фирмы ${RUN}`
  await api(request, '/employees', { full_name: cashier, is_cashier: true, pin: '1234' })
  const firm = `ОсОО Ждущая ${RUN}`
  const party = await api<{ id: string }>(request, '/parties', { kind: 'company', name: firm, phone: `0704${RUN.slice(-6)}` })
  const plate = `10KG${RUN.slice(-3)}AFW`
  const vehicle = await api<{ id: string }>(request, `/parties/${party.id}/vehicles`, { plate, brand: 'Isuzu', model: 'NPR' })
  // Машина ждёт подтверждения фирмы: так её отдаёт карточка клиента (статус ставит гараж фирмы).
  await page.route(`**/api/v1/parties/${party.id}/card`, async (route) => {
    const res = await route.fetch()
    const body = (await res.json()) as { vehicles: { id: string; status?: string }[] }
    for (const v of body.vehicles) if (v.id === vehicle.id) v.status = 'pending'
    await route.fulfill({ response: res, json: body })
  })
  await page.reload()

  await scan(page, code, 1)
  await shiftOnTill(page, cashier)
  if (await page.locator('#cashier-select').isVisible()) await choosePerson(page, 'cashier-select', cashier)
  await page.locator('[data-client-input]').fill(firm)
  await page.getByRole('button', { name: new RegExp(firm) }).first().click()
  await expect(page.getByLabel('Машина')).toHaveValue(vehicle.id)
  await expect(page.getByLabel('Машина').locator('option:checked')).toContainText('ждёт подтверждения фирмы')
  await expect(page.getByTestId('vehicle-pending')).toContainText(`Машина ${plate} ждёт подтверждения фирмы`)

  await openPay(page)
  await page.locator('[data-pay]').getByRole('button', { name: 'В долг', exact: true }).click()
  await expect(page.getByText(/машина не подтверждена фирмой — в долг нельзя/).first()).toBeVisible()
  await expect(page.getByRole('button', { name: 'Провести чек' }).first()).toBeDisabled()
  // Наличными — можно.
  await page.locator('[data-pay]').getByRole('button', { name: 'Наличные', exact: true }).click()
  await expect(page.getByText(/машина не подтверждена фирмой/)).toHaveCount(0)
})
