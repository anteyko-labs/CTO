// Касса без создания товаров и без минуса (задача 018, ADR-185): в поиске нет «Создать», в окне
// незнакомого кода нет «Новый товар» даже у владельца; товар с остатком 0 — «нет в наличии», в чеке
// красной строкой и «Провести чек» закрыт.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ensureShift } from './cashier'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2f5' }
const PICKER = /Сканируйте штрихкод/
const RUN = Date.now().toString().slice(-8)

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'POST', path: string, data?: unknown): Promise<T> {
  const res = await request.fetch(`/api/v1${path}`, { method, data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

test('касса: не заводит товар, нуль — «нет в наличии» и чек не провести', async ({ page }) => {
  await login(page)
  await ensureShift(page.request)
  const cat = await api<{ id: string }>(page.request, 'POST', '/categories', { name: `Остаток кассы ${RUN}`, kind: 'filter' })
  const name = `Фильтр без остатка ${RUN}`
  await api(page.request, 'POST', '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name,
    sale_price_tyiyn: 45_000,
  })
  await page.goto('/')
  const picker = page.getByPlaceholder(PICKER)

  // Поиск: нет «Создать», незнакомое — «Ничего не нашли».
  await picker.fill(`Такого нет ${RUN}`)
  await expect(page.getByText(`Ничего не нашли по «Такого нет ${RUN}»`)).toBeVisible()
  await expect(page.getByRole('button', { name: /Создать/ })).toHaveCount(0)

  // Незнакомый код у владельца: только привязка к существующему.
  const code = `44${RUN}05`
  await picker.fill(code)
  await picker.press('Enter')
  const dialog = page.getByRole('dialog', { name: `Код ${code} не найден` })
  await expect(dialog.getByRole('button', { name: 'Добавить к существующему товару' })).toBeVisible()
  await expect(dialog.getByRole('button', { name: 'Новый товар' })).toHaveCount(0)
  await page.keyboard.press('Escape')
  await expect(dialog).toBeHidden()

  // Остаток 0: в поиске «нет в наличии», в чеке — нехватка, «Провести чек» закрыт.
  await picker.fill(name)
  const row = page.getByRole('listitem').filter({ hasText: name })
  await expect(row).toContainText('нет в наличии')
  await row.click()
  await expect(page.getByTestId('cart')).toContainText(name)
  await expect(page.getByTestId('cart')).toContainText('больше, чем на складе')
  await expect(page.getByRole('button', { name: 'Провести чек' })).toBeDisabled()
})
