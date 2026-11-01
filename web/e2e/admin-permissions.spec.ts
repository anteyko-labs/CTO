// Права администраторов выбирает владелец (SPEC-30 §5, ADR-097, задача 003 Н8): экран «Что можно» в
// «Пользователи и права», без права пункты меню и кнопки скрыты, сервер отвечает «Нет права».
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
  await page.getByRole('link', { name: 'Пользователи и права', exact: true }).click()
  const row = page.getByRole('row', { name: new RegExp(admin) })
  await expect(row.getByRole('button', { name: '26 из 34' })).toBeVisible()
  await row.getByRole('button', { name: '26 из 34' }).click()
  const box = page.getByTestId('permissions')
  for (const group of ['Разделы', 'Касса', 'Деньги', 'Склад', 'Справочники']) await expect(box.getByRole('group', { name: group })).toBeVisible()
  await expect(box.getByTestId('owner-only')).toContainText('Себестоимость')
  await expect(box.getByLabel('Скидка в чеке')).not.toBeChecked()
  await expect(box.getByLabel('Расходы')).toBeChecked()
  // Коды на кассе (задача 005 Н3) — в «Кассе», выдано по умолчанию.
  await expect(box.getByRole('group', { name: 'Касса' }).getByLabel('Привязывать коды на кассе')).toBeChecked()
  await box.getByLabel('Расходы').uncheck()
  await box.getByLabel('Внести, изъять, перевести деньги').uncheck()
  await box.getByLabel('Скидка в чеке').check()
  await page.getByRole('button', { name: 'Сохранить' }).click()
  await expect(page.getByText('Права сохранены')).toBeVisible()
  await expect(row.getByRole('button', { name: '25 из 34' })).toBeVisible()

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

async function newAdmin(page: Page, tag: string) {
  const login = `${tag}${RUN}`
  const password = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  const res = await page.request.post('/api/v1/users', {
    data: { login, password, full_name: `${tag} ${RUN}`, role: 'admin' },
    headers: DEVICE,
  })
  expect(res.ok(), await res.text()).toBeTruthy()
  return { login, password }
}

test('группа целиком, «Как у…» и разделы меню; права доходят до открытого экрана без перезагрузки', async ({ page, browser }) => {
  await login(page, OWNER, OWNER_PASSWORD)
  const a = await newAdmin(page, 'razdel')
  const b = await newAdmin(page, 'kopiya')
  // Касса администратора Б открыта всё время.
  const second = await adminPage(browser, b.login, b.password)
  await expect(second.page.getByRole('link', { name: 'Поступления' })).toBeVisible()

  // Пункт меню «Пользователи и права» ведёт на тот же экран.
  await page.getByRole('link', { name: 'Пользователи и права' }).click()
  const rowA = page.getByRole('row', { name: new RegExp(a.login) })
  await rowA.getByRole('button', { name: '26 из 34' }).click()
  const box = page.getByTestId('permissions')
  // «Деньги»: «Включить всё», затем «Выключить всё».
  await box.getByTestId('permission-group-Деньги').click()
  await expect(box.getByTestId('permission-count')).toHaveText('30 из 34')
  await expect(box.getByTestId('permission-group-Деньги')).toHaveText('Выключить всё')
  await box.getByTestId('permission-group-Деньги').click()
  await expect(box.getByTestId('permission-count')).toHaveText('23 из 34')
  await box.getByLabel('Поступления').uncheck()
  await box.getByLabel('Долги: должники и долги поставщикам').uncheck()
  await expect(box.getByTestId('permission-count')).toHaveText('21 из 34')
  // Набор «По умолчанию» расставляет флажки, но сохраняет только «Сохранить».
  await box.getByRole('button', { name: 'По умолчанию' }).click()
  await expect(box.getByTestId('permission-count')).toHaveText('26 из 34')
  await box.getByTestId('permission-group-Деньги').click()
  await box.getByTestId('permission-group-Деньги').click()
  await box.getByLabel('Поступления').uncheck()
  await box.getByLabel('Долги: должники и долги поставщикам').uncheck()
  await page.getByRole('button', { name: 'Сохранить' }).click()
  await expect(page.getByText('Права сохранены')).toBeVisible()
  await expect(rowA.getByRole('button', { name: '21 из 34' })).toBeVisible()

  // Администратор А: ни денег, ни «Поступлений», ни «Долгов»; прямой адрес уводит на кассу.
  {
    const { ctx, page: p } = await adminPage(browser, a.login, a.password)
    for (const name of ['Расходы', 'Внести / Изъять', 'Поступления', 'Долги']) await expect(p.getByRole('link', { name, exact: true })).toHaveCount(0)
    await expect(p.getByRole('link', { name: 'Счета' })).toBeVisible()
    await p.goto('/income')
    await expect(p).toHaveURL(/\/$/)
    const denied = await p.request.get('/api/v1/reports/income')
    expect(denied.status()).toBe(403)
    expect(((await denied.json()) as { error: { message: string } }).error.message).toBe('Нет права: Поступления')
    await ctx.close()
  }

  // «Как у…» — администратору Б тот же набор.
  const rowB = page.getByRole('row', { name: new RegExp(b.login) })
  await rowB.getByRole('button', { name: '26 из 34' }).click()
  await box.getByTestId('permission-copy').selectOption({ label: `razdel ${RUN} (${a.login})` })
  await expect(box.getByTestId('permission-count')).toHaveText('21 из 34')
  await page.getByRole('button', { name: 'Сохранить' }).click()
  await expect(rowB.getByRole('button', { name: '21 из 34' })).toBeVisible()

  // Открытая касса Б: права перечитываются при возврате на вкладку (и раз в минуту) — без перезагрузки.
  await second.page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')))
  await expect(second.page.getByRole('link', { name: 'Поступления' })).toHaveCount(0)
  await expect(second.page.getByRole('link', { name: 'Расходы' })).toHaveCount(0)
  await second.ctx.close()
})
