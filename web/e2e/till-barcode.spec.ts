// Незнакомый код на кассе (задача 005 Н3, ADR-168): администратор только с правом «Привязывать коды
// на кассе» добавляет код к заведённому товару — товар в чеке, повторный скан кладёт его сразу; код,
// который тем временем занял другой товар, — предложение положить в чек его.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2e0' }
const PICKER = /Сканируйте штрихкод/
const RUN = Date.now().toString().slice(-8)

async function login(page: Page, user: string, password: string) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(user)
  await page.getByLabel('Пароль').fill(password)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'POST' | 'PUT', path: string, data: unknown): Promise<T> {
  const res = await request.fetch(`/api/v1${path}`, { method, data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

async function scan(page: Page, code: string) {
  const picker = page.getByPlaceholder(PICKER)
  await picker.fill(code)
  await picker.press('Enter')
}

test('касса: незнакомый код — «Добавить к существующему товару», товар в чеке, повторный скан — сразу', async ({ page, browser }) => {
  await login(page, OWNER, OWNER_PASSWORD)
  const cat = await api<{ id: string }>(page.request, 'POST', '/categories', { name: `Коды кассы ${RUN}`, kind: 'filter' })
  const product = (name: string, code: string) =>
    api<{ id: string }>(page.request, 'POST', '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name, barcodes: [code], sale_price_tyiyn: 45_000 })
  const filter = `Фильтр кассы ${RUN}`
  const other = `Фильтр другой ${RUN}`
  await product(filter, `41${RUN}01`)
  const otherProduct = await product(other, `41${RUN}02`)

  // Администратор только с правами кассы: без «Товары и штрихкоды», с «Привязывать коды на кассе».
  const login_ = `till${RUN}`
  const password = `pass-${RUN}-till`
  const admin = await api<{ id: string }>(page.request, 'POST', '/users', { login: login_, password, full_name: `Кассир кодов ${RUN}`, role: 'admin' })
  await api(page.request, 'PUT', `/users/${admin.id}/permissions`, {
    op_id: crypto.randomUUID(),
    permissions: ['sale_return', 'sale_debt', 'debt_payment', 'till_barcode', 'clients'],
  })

  const ctx = await browser.newContext()
  const a = await ctx.newPage()
  await login(a, login_, password)
  await a.goto('/')
  const cart = a.getByTestId('cart')

  // Незнакомый код: сразу кнопка «Добавить к существующему товару»; «Нового товара» без права «Товары» нет.
  const code = `42${RUN}03`
  await scan(a, code)
  const dialog = a.getByRole('dialog', { name: `Код ${code} не найден` })
  await expect(dialog).toBeVisible()
  await expect(dialog.getByRole('button', { name: 'Добавить к существующему товару' })).toBeVisible()
  await expect(dialog.getByRole('button', { name: 'Новый товар' })).toHaveCount(0)
  await dialog.getByPlaceholder('Название, бренд, артикул или другой код').fill(filter)
  const row = dialog.getByRole('listitem').filter({ hasText: filter })
  await expect(row).toContainText(`41${RUN}01`)
  await row.getByRole('button', { name: 'Привязать код' }).click()
  await expect(dialog).toBeHidden()
  await expect(cart).toContainText(filter)

  // Повторный скан того же кода — товар сразу, без окна.
  await scan(a, code)
  await expect(a.getByRole('textbox', { name: 'Кол-во' })).toHaveValue('2')
  await expect(a.getByRole('dialog')).toHaveCount(0)

  // Код, который тем временем привязали к другому товару: предложение положить в чек его.
  const raced = `43${RUN}04`
  await scan(a, raced)
  const second = a.getByRole('dialog', { name: `Код ${raced} не найден` })
  await expect(second).toBeVisible()
  await api(page.request, 'POST', `/products/${otherProduct.id}/barcodes`, { op_id: crypto.randomUUID(), code: raced })
  await second.getByPlaceholder('Название, бренд, артикул или другой код').fill(filter)
  await second.getByRole('listitem').filter({ hasText: filter }).getByRole('button', { name: 'Привязать код' }).click()
  await expect(second.getByTestId('unknown-taken')).toContainText(`Этот код уже у «${other}»`)
  await second.getByRole('button', { name: 'Положить в чек' }).click()
  await expect(second).toBeHidden()
  await expect(cart).toContainText(other)
  await ctx.close()
})
