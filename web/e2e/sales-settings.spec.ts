// «Настройки → Продажи» (задача 003): «Цену новой услуги задаёт только владелец» — по умолчанию нет;
// включено — администратор без права «Цены товаров и услуг» заводит услугу с ценой 0 (поле цены
// неактивно, сервер не примет другую цену). О каждой услуге администратора владельцу — уведомление.
// В конце настройка как была.
import { expect, test, type Browser, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e3f7' }
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

test('цена новой услуги: настройка владельца, администратор — с ценой 0, уведомление владельцу', async ({ page, browser }) => {
  await login(page, OWNER, OWNER_PASSWORD)
  const admin = `usl${RUN}`
  const password = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  const res = await page.request.post('/api/v1/users', { data: { login: admin, password, full_name: `Услуги ${RUN}`, role: 'admin' }, headers: DEVICE })
  expect(res.ok(), await res.text()).toBeTruthy()

  // Экран настройки: по умолчанию выключено; включили — сохранилось и после перезагрузки.
  await page.goto('/settings')
  await page.getByRole('link', { name: /Кто задаёт цену новой услуги/ }).click()
  await expect(page.getByRole('heading', { name: 'Продажи', exact: true })).toBeVisible()
  const box = page.getByLabel('Цену новой услуги задаёт только владелец')
  await expect(box).not.toBeChecked()
  await box.check()
  await expect(page.getByText('Цену новой услуги задаёт только владелец').last()).toBeVisible()
  await page.reload()
  await expect(page.getByLabel('Цену новой услуги задаёт только владелец')).toBeChecked()

  // Администратор: поле цены неактивно, услуга заводится с ценой 0.
  const { ctx, page: a } = await adminPage(browser, admin, password)
  await a.goto('/services')
  await a.getByRole('button', { name: /Добавить|Новая услуга/ }).first().click()
  await expect(a.locator('#service-price')).toBeDisabled()
  await expect(a.locator('#service-price')).toHaveValue('0')
  const name = `Промывка ${RUN}`
  await a.locator('#service-name').fill(name)
  await a.getByRole('dialog', { name: 'Новая услуга' }).getByRole('button', { name: /Добавить|Сохранить|Завести/ }).last().click()
  await expect(a.getByRole('row', { name: new RegExp(name) })).toBeVisible()
  // Сервер не примет цену в обход экрана (I12).
  const direct = await a.request.post('/api/v1/services', { data: { name: `Обход ${RUN}`, price_tyiyn: 10_000 }, headers: DEVICE })
  expect(direct.status()).toBe(403)
  await ctx.close()

  // Владельцу — уведомление «Администратор завёл услугу».
  await page.goto('/notifications')
  await expect(page.getByText(`Администратор завёл услугу «${name}»`).first()).toBeVisible()

  // Как было.
  const back = await page.request.put('/api/v1/settings/sales', { data: { service_price_owner_only: false }, headers: DEVICE })
  expect(back.ok(), await back.text()).toBeTruthy()
})
