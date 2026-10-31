// Сообщения клиентам (SPEC-32): экран «Настройки → Сообщения клиентам» — шаблоны WhatsApp и их
// переменные по порядку, журнал; в карточке фирмы — руководитель, флажок «Не присылать сообщения
// о закупах» и ссылка на бот у владельца. Без ключей WhatsApp сервер ничего не отправляет.
import { expect, test, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e32a' }
const RUN = Date.now().toString().slice(-8)

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

test('шаблоны WhatsApp, журнал и настройки фирмы', async ({ page }) => {
  await login(page)
  await page.goto('/settings')
  await page.getByRole('link', { name: /Сообщения клиентам/ }).click()
  await expect(page.getByRole('heading', { name: 'Сообщения клиентам' })).toBeVisible()
  // Переменные шаблонов — по порядку, текст для Meta — на экране.
  await expect(page.getByText('{{1}} — имя клиента')).toBeVisible()
  await expect(page.getByText('{{7}} — ссылка на кабинет или на бот')).toBeVisible()
  await expect(page.getByText(/Не забудьте поменять \{\{2\}\} на \{\{3\}\} на пробеге \{\{4\}\}/)).toBeVisible()

  // Неверное имя шаблона — подсказка и «Сохранить» недоступна.
  await page.locator('#oil-template').fill('Замена масла')
  await expect(page.getByText('строчные латинские буквы, цифры и «_»')).toBeVisible()
  await expect(page.getByRole('button', { name: 'Сохранить' }).first()).toBeDisabled()
  await page.locator('#oil-template').fill('avtodom_oil_change')
  await page.locator('#purchase-template').fill('avtodom_purchase')
  await page.locator('#km-per-day').fill('45')
  await page.getByRole('button', { name: 'Сохранить' }).first().click()
  await expect(page.getByText('Сохранено')).toBeVisible()
  await page.reload()
  await expect(page.locator('#oil-template')).toHaveValue('avtodom_oil_change')
  await expect(page.locator('#km-per-day')).toHaveValue('45')
  await expect(page.getByText('Журнал сообщений')).toBeVisible()

  // Фирма: руководитель, флажок закупов, ссылка на бот.
  const res = await page.request.post('/api/v1/parties', {
    data: { role: 'customer', kind: 'company', name: `ОсОО Такси ${RUN}`, phone: '0555 10 20 30' },
    headers: DEVICE,
  })
  expect(res.ok(), await res.text()).toBeTruthy()
  const firm = (await res.json()) as { id: string }
  await page.goto(`/clients/${firm.id}`)
  await page.getByRole('button', { name: '+ работник' }).click()
  await page.locator('#contact-name').fill('Азиз')
  await page.locator('#contact-phone').fill('0700 12 34 56')
  await page.getByLabel(/Руководитель/).check()
  await page.getByRole('button', { name: 'Добавить', exact: true }).click()
  await expect(page.getByText('руководитель', { exact: true })).toBeVisible()
  const flag = page.locator('#party-no-purchase-messages')
  await expect(flag).not.toBeChecked()
  await flag.click()
  await expect(page.getByText('Сообщения о закупах отключены')).toBeVisible()
  await expect(flag).toBeChecked()
  await page.reload()
  await expect(page.locator('#party-no-purchase-messages')).toBeChecked()
  await expect(page.getByTestId('bot-link')).toBeVisible()
  await expect(page.getByTestId('bot-link').getByRole('button', { name: 'Новая ссылка' })).toBeVisible()
})

// Задача 002, волна 2 (П7, П7б, П9): «Должникам» — выключатели, тексты с подстановками и образцом.
test('должникам: «Получили оплату», раз в месяц и напоминание о сроке', async ({ page }) => {
  await login(page)
  await page.goto('/settings/client-messages')
  const card = page.getByTestId('debtor-messages')
  await expect(card).toBeVisible()
  // По умолчанию — как в задаче: включено, 1-го числа в 10:00, тексты с приветствием.
  await expect(card.getByRole('checkbox', { name: /Получили оплату/ })).toBeChecked()
  await expect(card.getByRole('checkbox', { name: /Раз в месяц/ })).toBeChecked()
  await expect(page.locator('#monthly-day')).toHaveValue('1')
  await expect(page.locator('#monthly-hour')).toHaveValue('10')
  await expect(page.locator('#payment-text')).toHaveValue('Здравствуйте, {имя}! Получили оплату {сумма}. Долг на сегодня — {долг}.')
  await expect(page.getByTestId('payment-text-sample')).toContainText('Здравствуйте, Азиз! Получили оплату 5 000 сом.')
  await expect(page.locator('#debt-due-text')).toHaveValue(/^Здравствуйте, \{имя\}! Напоминаем/)
  // Шаблоны Meta для юрлиц — на экране.
  await expect(page.getByText(/Получили оплату \{\{2\}\}\. Долг на сегодня — \{\{3\}\}/)).toBeVisible()
  // Непонятная подстановка — подсказка и «Сохранить» недоступна.
  await page.locator('#payment-text').fill('Привет, {телефон}!')
  await expect(card.getByText('непонятная подстановка {телефон}')).toBeVisible()
  await expect(card.getByRole('button', { name: 'Сохранить' })).toBeDisabled()
  await page.locator('#payment-text').fill(`{имя}, спасибо! Приняли {сумма}, осталось {долг}. ${RUN}`)
  await expect(page.getByTestId('payment-text-sample')).toContainText('Азиз, спасибо! Приняли 5 000 сом, осталось 1 240 сом.')
  await page.locator('#monthly-day').fill('5')
  await card.getByRole('button', { name: 'Сохранить' }).click()
  await expect(page.getByText('Сохранено')).toBeVisible()
  await page.reload()
  await expect(page.locator('#payment-text')).toHaveValue(`{имя}, спасибо! Приняли {сумма}, осталось {долг}. ${RUN}`)
  await expect(page.locator('#monthly-day')).toHaveValue('5')
  // Вернуть как было.
  await page.locator('#payment-text').fill('Здравствуйте, {имя}! Получили оплату {сумма}. Долг на сегодня — {долг}.')
  await page.locator('#monthly-day').fill('1')
  await page.getByTestId('debtor-messages').getByRole('button', { name: 'Сохранить' }).click()
  await expect(page.getByText('Сохранено')).toBeVisible()
})
