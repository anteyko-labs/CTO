// Задача 003 Н10: права кассира без входа (ADR-152) и характеристики объектов (ADR-153):
// «Что можно на кассе» у кассира, реквизиты поставщика, «Скрыть категорию», перенос машины.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e803' }
const RUN = Date.now().toString().slice(-8)

async function login(page: Page, user: string, password: string) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(user)
  await page.getByLabel('Пароль').fill(password)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'post' | 'put' | 'patch', path: string, data: unknown): Promise<T> {
  const res = await request[method](`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), await res.text()).toBeTruthy()
  return (await res.json()) as T
}

/** Закрыть открытую смену, если она есть: пересчитано ровно столько, сколько должно быть. */
async function closeOpenShift(request: APIRequestContext) {
  const cur = (await (await request.get('/api/v1/shifts/current')).json()) as { id: string; expected_tyiyn: number } | null
  if (!cur) return
  await api(request, 'post', `/shifts/${cur.id}/close`, { op_id: crypto.randomUUID(), counted_tyiyn: cur.expected_tyiyn, comment: 'e2e' })
  await api(request, 'post', `/shifts/${cur.id}/handover`, { op_id: crypto.randomUUID(), amount_tyiyn: cur.expected_tyiyn })
}

test('кассир без «Расходов»: в его смену администратор не видит кнопку, сервер отвечает с именем кассира', async ({ page, browser }) => {
  await login(page, OWNER, OWNER_PASSWORD)
  const name = `Нурлан ${RUN}`
  const cashier = await api<{ id: string }>(page.request, 'post', '/employees', { full_name: name, is_cashier: true, pin: '4321' })
  const admin = `kassa${RUN}`
  const password = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  await api(page.request, 'post', '/users', { login: admin, password, full_name: `Касса ${RUN}`, role: 'admin' })

  // Владелец: «Сотрудники» → кассир → «Что можно на кассе» — снять «Расходы».
  await page.goto('/employees')
  await page.getByRole('row', { name: new RegExp(name) }).click()
  const box = page.getByTestId('cashier-rights')
  await expect(box.getByText('как у входа')).toBeVisible()
  await expect(box.getByLabel('Расходы')).toBeChecked()
  await box.getByLabel('Расходы').uncheck()
  await box.getByRole('button', { name: 'Сохранить права' }).click()
  await expect(page.getByText('Права кассира сохранены')).toBeVisible()
  await expect(box.getByText('снято: 1')).toBeVisible()

  await closeOpenShift(page.request)
  const shift = await api<{ id: string }>(page.request, 'post', '/shifts/open', {
    op_id: crypto.randomUUID(),
    cashier_employee_id: cashier.id,
    cashier_pin: '4321',
  })
  try {
    const ctx = await browser.newContext()
    const a = await ctx.newPage()
    await login(a, admin, password)
    await a.goto('/')
    await expect(a.getByRole('button', { name: 'Принять оплату долга' }).first()).toBeVisible()
    await expect(a.getByRole('button', { name: /Мелкий расход/ })).toHaveCount(0)
    const res = await a.request.post('/api/v1/expenses', {
      data: { op_id: crypto.randomUUID(), article_id: crypto.randomUUID(), amount_tyiyn: 1000 },
      headers: DEVICE,
    })
    expect(res.status()).toBe(403)
    expect(((await res.json()) as { error: { message: string } }).error.message).toBe(`Нет права у кассира ${name}: Расходы`)
    await ctx.close()
  } finally {
    await closeOpenShift(page.request)
    expect(shift.id).toBeTruthy()
  }
})

test('поставщик: ИНН, адрес, банк и срок оплаты правятся и видны в карточке', async ({ page }) => {
  await login(page, OWNER, OWNER_PASSWORD)
  const name = `ОсОО Реквизиты ${RUN}`
  await page.goto('/suppliers')
  await page.getByRole('button', { name: 'Добавить' }).first().click()
  await page.locator('#supplier-name').fill(name)
  await page.locator('#supplier-inn').fill('01234567890123')
  await page.getByRole('button', { name: 'Добавить' }).last().click()
  await expect(page.getByText('Поставщик добавлен')).toBeVisible()
  const row = page.getByRole('row', { name: new RegExp(name) })
  await expect(row.getByText('01234567890123')).toBeVisible()
  await row.click()
  await page.getByRole('button', { name: 'Изменить' }).click()
  await page.locator('#supplier-address').fill('Бишкек, Жибек Жолу 1')
  await page.locator('#supplier-bank').fill('Оптима Банк, БИК 109018')
  await page.locator('#supplier-account').fill('1090000000000001')
  await page.locator('#supplier-due').fill('14')
  await page.getByRole('button', { name: 'Сохранить' }).click()
  const req = page.getByTestId('supplier-requisites')
  await expect(req.getByText('ИНН 01234567890123')).toBeVisible()
  await expect(req.getByText('Бишкек, Жибек Жолу 1')).toBeVisible()
  await expect(req.getByText('Срок оплаты: 14 дн.')).toBeVisible()
})

test('«Скрыть категорию»: с товаром в продаже — нельзя, пустая скрывается и пропадает из выбора', async ({ page }) => {
  await login(page, OWNER, OWNER_PASSWORD)
  const name = `Скрываемая ${RUN}`
  const cat = await api<{ id: string }>(page.request, 'post', '/categories', { name, kind: 'other' })
  const p = await api<{ id: string }>(page.request, 'post', '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Щётка ${RUN}`,
    sale_price_tyiyn: 30_000,
  })
  await page.goto('/categories')
  await page.getByRole('button', { name: `Скрыть категорию ${name}` }).click()
  await expect(page.getByText(/в категории товаров в продаже: 1/)).toBeVisible()
  await api(page.request, 'patch', `/products/${p.id}`, { archived: true })
  await page.getByRole('button', { name: `Скрыть категорию ${name}` }).click()
  await expect(page.getByText(`Категория «${name}» скрыта`)).toBeVisible()
  await expect(page.getByRole('row', { name: new RegExp(name) }).getByText('скрыта')).toBeVisible()
  const list = (await (await page.request.get('/api/v1/categories')).json()) as { id: string }[]
  expect(list.some((c) => c.id === cat.id)).toBe(false)
  await page.getByRole('button', { name: `Показать категорию ${name}` }).click()
  await expect(page.getByText(`Категория «${name}» снова в выборе`)).toBeVisible()
})

test('машина переходит к другому клиенту вместе с книжкой', async ({ page }) => {
  await login(page, OWNER, OWNER_PASSWORD)
  const person = (first: string, phone: string) => ({ role: 'customer', kind: 'person', first_name: first, last_name: `Перенос${RUN}`, phone })
  const from = await api<{ id: string }>(page.request, 'post', '/parties', person('Бакыт', `+99655${RUN.slice(-7)}`))
  await api(page.request, 'post', '/parties', person('Нурбек', `+99677${RUN.slice(-7)}`))
  const plate = `01KG${RUN.slice(-3)}MOV`
  const v = await api<{ id: string }>(page.request, 'post', `/parties/${from.id}/vehicles`, { plate, brand: 'Toyota', model: 'Camry' })
  await api(page.request, 'post', `/vehicles/${v.id}/oil-changes`, { change_date: '2026-10-01', mileage_km: 120000, oil_text: `Mobil ${RUN}` })

  await page.goto(`/clients/${from.id}`)
  await page.getByText(plate, { exact: true }).locator('..').getByRole('button', { name: 'Изменить' }).click()
  await page.getByRole('button', { name: 'Перенести к другому клиенту' }).click()
  const box = page.getByTestId('vehicle-move')
  await box.locator('#vehicle-move-q').fill(`Перенос${RUN}`)
  // Прежний клиент в списке не предлагается — только другой.
  await box.getByRole('button', { name: /Нурбек/ }).click()
  await expect(box.getByText(/Новый клиент: .*Нурбек/)).toBeVisible()
  await box.getByRole('button', { name: 'Перенести' }).click()
  await expect(page.getByText(`Машина ${plate} перенесена к клиенту`)).toBeVisible()
  await expect(page.getByText(plate)).toHaveCount(0)
})
