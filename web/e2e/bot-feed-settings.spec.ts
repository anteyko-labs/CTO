// «Настройки → Уведомления в бот» (задача 002 П11, ADR-138): владелец выключает событие и
// выбирает режим чеков; после перезагрузки всё на месте. В конце — как было.
import { expect, test } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'

test('уведомления в бот: выключатель события и режим чеков сохраняются', async ({ page }) => {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
  // Повтор после сбоя начинает с настройки по умолчанию.
  const reset = await page.request.put('/api/v1/settings/bot-feed', {
    data: { op_id: crypto.randomUUID(), off: [], sales: 'none' },
    headers: { 'x-device-id': crypto.randomUUID() },
  })
  expect(reset.ok()).toBeTruthy()

  await page.goto('/settings')
  await page.getByRole('link', { name: /Уведомления в бот/ }).click()
  await expect(page.getByRole('heading', { name: 'Уведомления в бот' })).toBeVisible()

  // По умолчанию чеки не присылаются, события смены включены.
  const sales = page.getByTestId('bot-feed-sales')
  await expect(sales.getByRole('radio', { name: 'Не присылать' })).toBeChecked()
  const shift = page.getByTestId('bot-feed-group-shift')
  const open = shift.getByRole('checkbox', { name: 'Смена открыта' })
  await expect(open).toBeChecked()
  for (const g of ['Касса', 'Склад', 'Деньги', 'Долги', 'Зарплата', 'Смена', 'Прочее']) {
    await expect(page.getByRole('heading', { name: g, exact: true })).toBeVisible()
  }

  await open.uncheck()
  await expect(page.getByText('«Смена открыта» — не приходит в бот')).toBeVisible()
  await expect(open).not.toBeChecked()
  await sales.getByRole('radio', { name: 'Только в долг и возвраты' }).check()
  await expect(page.getByText('Чеки: только в долг и возвраты')).toBeVisible()

  await page.reload()
  await expect(page.getByTestId('bot-feed-sales').getByRole('radio', { name: 'Только в долг и возвраты' })).toBeChecked()
  await expect(page.getByTestId('bot-feed-group-shift').getByRole('checkbox', { name: 'Смена открыта' })).not.toBeChecked()
  await expect(page.getByTestId('bot-feed-group-shift').getByText('включено 6 из 7')).toBeVisible()

  // Вернуть как было.
  await page.getByTestId('bot-feed-group-shift').getByRole('checkbox', { name: 'Смена открыта' }).check()
  await expect(page.getByText('«Смена открыта» — приходит в бот')).toBeVisible()
  await page.getByTestId('bot-feed-sales').getByRole('radio', { name: 'Не присылать' }).check()
  await expect(page.getByText('Чеки: не присылать')).toBeVisible()
})
