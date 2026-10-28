// Права администраторов выбирает владелец (SPEC-30 §5, ADR-097): экран «Что можно» в
// «Настройки» → «Пользователи», без права пункты меню и кнопки скрыты, сервер отвечает «Нет права».
import { expect, test, type Browser, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e305' }
const RUN = Date.now().toString().slice(-8)

async function login(page: Page, user: string, password: string) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(user)
  await page.getByLabel('Пароль').fill(password)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function adminPage(browser: Browser, user: string, password: string) {
  const ctx = await browser.newContext()
  const page = await ctx.newPage()
  await login(page, user, password)
  return { ctx, page }
}

test('владелец выбирает, что можно администратору; без права — ни пункта меню, ни действия', async ({ page, browser }) => {
  const admin = `prava${RUN}`
  const password = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  await login(page, OWNER, OWNER_PASSWORD)
  const res = await page.request.post('/api/v1/users', {
    data: { login: admin, password, full_name: `Эрбол ${RUN}`, role: 'admin' },
    headers: DEVICE,
  })
  expect(res.ok(), await res.text()).toBeTruthy()

  // Новый администратор — с правами по умолчанию: «Расходы» и «Внести / Изъять» у него есть.
  {
    const { ctx, page: a } = await adminPage(browser, admin, password)
    await expect(a.getByRole('link', { name: 'Расходы' })).toBeVisible()
    await expect(a.getByRole('link', { name: 'Внести / Изъять' })).toBeVisible()
    await expect(a.getByRole('link', { name: 'Перелив' })).toHaveCount(0)
    await ctx.close()
  }

  // «Настройки» → «Пользователи» → администратор → «Что можно».
  await page.getByRole('link', { name: 'Настройки' }).click()
  await page.getByRole('link', { name: /Пользователи/ }).click()
  const row = page.getByRole('row', { name: new RegExp(admin) })
  await expect(row.getByRole('button', { name: '14 из 18' })).toBeVisible()
  await row.getByRole('button', { name: '14 из 18' }).click()
  const box = page.getByTestId('permissions')
  for (const group of ['Касса', 'Деньги', 'Склад', 'Справочники']) await expect(box.getByRole('group', { name: group })).toBeVisible()
  await expect(box.getByLabel('Скидка в чеке')).not.toBeChecked()
  await expect(box.getByLabel('Расходы')).toBeChecked()
  await box.getByLabel('Расходы').uncheck()
  await box.getByLabel('Внести, изъять, перевести деньги').uncheck()
  await box.getByLabel('Скидка в чеке').check()
  await page.getByRole('button', { name: 'Сохранить' }).click()
  await expect(page.getByText('Права сохранены')).toBeVisible()
  await expect(row.getByRole('button', { name: '13 из 18' })).toBeVisible()

  // Администратор без права: пунктов меню нет, сервер отвечает «Нет права: Расходы».
  const { ctx, page: a } = await adminPage(browser, admin, password)
  await expect(a.getByRole('link', { name: 'Расходы' })).toHaveCount(0)
  await expect(a.getByRole('link', { name: 'Внести / Изъять' })).toHaveCount(0)
  await expect(a.getByRole('link', { name: 'Чеки' })).toBeVisible()
  const me = (await (await a.request.get('/api/v1/auth/me')).json()) as { permissions: string[] }
  expect(me.permissions).toContain('discount')
  expect(me.permissions).not.toContain('expenses')
  const articles = (await (await a.request.get('/api/v1/expense-articles')).json()) as { id: string; name: string }[]
  const denied = await a.request.post('/api/v1/expenses', {
    data: { op_id: crypto.randomUUID(), article_id: articles[0].id, amount_tyiyn: 100, comment: 'проверка' },
    headers: DEVICE,
  })
  expect(denied.status()).toBe(403)
  expect(((await denied.json()) as { error: { message: string } }).error.message).toBe('Нет права: Расходы')
  // Счета он видит, а «Внести» и «Изъять» на них — нет.
  await a.goto('/accounts')
  await expect(a.getByTestId('money-actions').getByRole('button', { name: 'Внести' })).toHaveCount(0)
  await ctx.close()

  // Журнал: владельцу — уведомление о правах.
  await page.goto('/notifications')
  await expect(page.getByText(`Права администратора: Эрбол ${RUN}`).first()).toBeVisible()
  await expect(page.getByText(/включено: Скидка в чеке; выключено: Внести, изъять, перевести деньги, Расходы/).first()).toBeVisible()
})
