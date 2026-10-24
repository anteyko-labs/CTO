// Несколько филиалов в браузере (SPEC-33): владелец заводит филиал в «Настройки → Филиалы»,
// переключает филиал в шапке, видит свои кассы филиала и «Все филиалы» на сводке с разбивкой;
// администратор нового филиала видит только свой филиал. Файл — последним: после него в базе
// несколько филиалов.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2eb' }
const RUN = Date.now().toString().slice(-8)

async function login(page: Page, login = OWNER, password = OWNER_PASSWORD) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(login)
  await page.getByLabel('Пароль').fill(password)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'GET' | 'POST' | 'PATCH' | 'PUT', path: string, data?: unknown, branch?: string): Promise<T> {
  const res = await request.fetch(`/api/v1${path}`, { method, data, headers: { ...DEVICE, ...(branch ? { 'X-Branch-Id': branch } : {}) } })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

test('филиалы: завести, переключить, свои кассы, «Все филиалы», администратор филиала', async ({ page, browser }) => {
  await login(page)
  const name = `Филиал E2E ${RUN}`
  await page.goto('/settings')
  await page.getByRole('link', { name: /Филиалы/ }).click()
  await expect(page.getByRole('heading', { name: 'Филиалы' })).toBeVisible()
  await page.getByRole('button', { name: 'Новый филиал' }).click()
  await page.getByLabel(/Название/).fill(name)
  await page.getByLabel('Адрес').fill('ул. Проверочная, 1')
  await page.getByRole('button', { name: 'Завести филиал' }).click()
  await expect(page.getByText(/Филиал заведён/)).toBeVisible()
  await expect(page.getByTestId('branch-row').filter({ hasText: name })).toContainText('работает')

  // Переключатель в шапке — после перезагрузки (у владельца теперь несколько филиалов).
  await page.reload()
  const sw = page.getByTestId('branch-switch').first()
  await expect(sw).toBeVisible()
  const branches = await api<{ id: string; name: string; is_main: boolean }[]>(page.request, 'GET', '/branches')
  const created = branches.find((b) => b.name === name)
  const main = branches.find((b) => b.is_main)
  expect(created && main).toBeTruthy()
  await sw.selectOption(created?.id ?? '')
  await expect(page.getByTestId('branch-switch').first()).toHaveValue(created?.id ?? '')
  // Свои кассы нового филиала: «Касса» без движений.
  await page.goto('/accounts')
  await expect(page.getByRole('heading', { name: 'Обзор счетов' })).toBeVisible()
  await expect(page.getByTestId('account-row').filter({ hasText: 'Касса' }).first()).toBeVisible()

  // Администратор нового филиала: видит только свой филиал, переключателя нет.
  const login2 = `adm${RUN}`
  const pass = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  await api(page.request, 'POST', '/users', { login: login2, password: pass, full_name: `Админ ${RUN}`, role: 'admin' }, created?.id)
  const ctx = await browser.newContext()
  const admin = await ctx.newPage()
  await login(admin, login2, pass)
  await expect(admin.getByTestId('branch-name').first()).toContainText(name)
  await expect(admin.getByTestId('branch-switch')).toHaveCount(0)
  const mine = await api<{ id: string }[]>(admin.request, 'GET', '/branches')
  expect(mine.map((b) => b.id)).toEqual([created?.id])
  await ctx.close()

  // Сводка «Все филиалы»: разбивка по филиалам с обоими названиями.
  await page.getByTestId('branch-switch').first().selectOption(main?.id ?? '')
  await expect(page.getByTestId('branch-switch').first()).toHaveValue(main?.id ?? '')
  await page.goto('/summary')
  await page.getByTestId('scope-all').click()
  const breakdown = page.getByTestId('branch-breakdown')
  await expect(breakdown).toContainText(name)
  await expect(breakdown).toContainText(main?.name ?? '')
  await expect(breakdown).toContainText('Все филиалы')
  await page.getByTestId('scope-branch').click()
  await expect(page.getByTestId('branch-breakdown')).toHaveCount(0)

  // Закрыть филиал: история остаётся, изменения в нём недоступны.
  await page.goto('/settings/branches')
  const row = page.getByTestId('branch-row').filter({ hasText: name })
  await row.getByRole('button', { name: 'Ещё действия' }).click()
  await page.getByRole('menuitem', { name: 'Закрыть филиал' }).click()
  await expect(row).toContainText('закрыт')
  const res = await page.request.fetch('/api/v1/expense-articles', {
    method: 'POST',
    data: { name: `Статья ${RUN}` },
    headers: { ...DEVICE, 'X-Branch-Id': created?.id ?? '' },
  })
  expect(res.status()).toBe(409)
})
