// Главный путь этапа 1 в браузере: товар → приход → продажа → возврат → остатки, и права администратора.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2e0' }
const PICKER = 'Сканируйте штрихкод или введите название'

/** Уникальная метка прогона, чтобы тесты не мешали данным в базе. */
const RUN = Date.now().toString().slice(-8)
const money = (text: string) => new RegExp(text.replace(' ', '\\s'))

async function login(page: Page, user: string, password: string) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(user)
  await page.getByLabel('Пароль').fill(password)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function apiPost<T>(request: APIRequestContext, path: string, data: unknown): Promise<T> {
  const res = await request.post(`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

async function scan(page: Page, code: string) {
  const picker = page.getByPlaceholder(PICKER)
  await picker.fill(code)
  await picker.press('Enter')
}

test('товар, приход, продажа, возврат и остаток', async ({ page }) => {
  const code = `29${RUN}123`
  const cashier = `Кассир ${RUN}`
  const category = `Фильтры ${RUN}`
  const product = `Фильтр E2E ${RUN}`

  await login(page, OWNER, OWNER_PASSWORD)
  await apiPost(page.request, '/employees', { full_name: cashier, is_cashier: true, is_master: true })
  await apiPost(page.request, '/categories', { name: category, kind: 'filter' })

  // Приход: незнакомый код открывает создание товара.
  await page.goto('/receipts/new')
  await scan(page, code)
  const modal = page.getByRole('heading', { name: 'Новый товар' })
  await expect(modal).toBeVisible()
  await page.getByLabel('Категория').selectOption({ label: category })
  await page.getByLabel('Название').first().fill(product)
  await page.getByLabel('Цена продажи, с').fill('500')
  await page.getByRole('button', { name: 'Сохранить' }).click()
  await expect(modal).toBeHidden()

  await page.getByLabel('Кол-во, шт').fill('10')
  await page.getByLabel('Цена за шт, с').fill('300')
  const print = page.getByLabel('Печатать этикетки')
  if (await print.isChecked()) await print.uncheck()
  await page.getByRole('button', { name: 'Провести приход' }).click()
  await expect(page).toHaveURL(/\/receipts\/[0-9a-f-]{36}$/)
  await expect(page.getByText(money('3 000,00')).first()).toBeVisible()

  // Продажа за наличные со сдачей.
  await page.getByRole('link', { name: 'Касса' }).click()
  await page.getByRole('combobox', { name: /^Кассир/ }).selectOption({ label: cashier })
  await scan(page, code)
  await expect(page.getByText(product)).toBeVisible()
  // Касса с одним кассиром выбирает его сама; быстрая кнопка подставляет купюру.
  await page.getByRole('button', { name: money('1 000,00 с') }).click()
  await expect(page.getByLabel('Получено, с')).toHaveValue('1000,00')
  await expect(page.getByText('Сдача', { exact: true }).locator('..')).toContainText(money('500,00'))
  await page.getByRole('button', { name: 'Провести чек' }).click()
  await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()

  // Возврат одной штуки.
  await page.getByRole('link', { name: 'Открыть' }).click()
  await expect(page.getByRole('heading', { name: /^Чек № \d+$/ })).toBeVisible()
  await page.getByRole('button', { name: 'Возврат' }).click()
  await page.getByRole('row', { name: new RegExp(product) }).getByRole('textbox').fill('1')
  await page.getByLabel('Причина').fill('проверка возврата')
  await expect(page.getByText(money('К возврату: 500,00'))).toBeVisible()
  await page.getByRole('button', { name: 'Оформить возврат' }).click()
  await expect(page.getByRole('heading', { name: /^Возврат № \d+$/ })).toBeVisible()

  // Остаток: 10 пришло, 1 продано, 1 вернули.
  await page.getByRole('link', { name: 'Остатки' }).click()
  await expect(page.getByRole('row', { name: new RegExp(product) })).toContainText('10 шт')

  // Сверка остатков с движениями.
  await page.getByRole('button', { name: 'Сверить остатки' }).click()
  await expect(page.getByText('Расхождений нет')).toBeVisible()
})

test('администратор не видит пользователей и сверку остатков', async ({ page, browser }) => {
  const admin = `admin${RUN}`
  const adminPassword = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  await login(page, OWNER, OWNER_PASSWORD)
  await apiPost(page.request, '/users', { login: admin, password: adminPassword, full_name: `Админ ${RUN}`, role: 'admin' })

  const ctx = await browser.newContext()
  const adminPage = await ctx.newPage()
  await login(adminPage, admin, adminPassword)
  await expect(adminPage.getByRole('link', { name: 'Пользователи' })).toHaveCount(0)
  await adminPage.getByRole('link', { name: 'Остатки' }).click()
  await expect(adminPage.getByRole('heading', { name: 'Остатки' })).toBeVisible()
  await expect(adminPage.getByRole('button', { name: 'Сверить остатки' })).toHaveCount(0)
  const res = await adminPage.request.get('/api/v1/users')
  expect(res.status()).toBe(403)
  await ctx.close()
})
