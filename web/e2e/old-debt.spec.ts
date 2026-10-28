// Старый долг из тетради (SPEC-38): «Долги» → «Занести старый долг» → новый клиент, сумма, дата →
// строка в «Должны нам» с просрочкой; «Занести и ещё» сохраняет дату, клиент и сумма очищаются.
import { expect, test, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const RUN = Date.now().toString().slice(-8)

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

/** Дата YYYY-MM-DD за `days` дней до сегодняшней в Бишкеке. */
function daysAgo(days: number): string {
  const today = new Date().toLocaleDateString('sv-SE', { timeZone: 'Asia/Bishkek' })
  const d = new Date(`${today}T12:00:00Z`)
  d.setUTCDate(d.getUTCDate() - days)
  return d.toISOString().slice(0, 10)
}

test('старый долг новой фирме: срок от даты долга, «Занести и ещё» сохраняет дату', async ({ page }) => {
  await login(page)
  await page.goto('/debts')
  await page.getByRole('button', { name: 'Занести старый долг' }).click()
  const modal = page.getByRole('dialog', { name: 'Занести старый долг' })
  await expect(modal).toBeVisible()

  // Новая фирма: срок оплаты 14 дней по умолчанию — долг от 60 дней назад просрочен на 47 дн.
  const firm = `Тетрадь ${RUN}`
  await modal.getByRole('button', { name: '+ Новый клиент' }).click()
  await modal.getByRole('button', { name: 'Фирма' }).click()
  await modal.locator('#old-debt-name').fill(firm)
  await modal.locator('#old-debt-sum').fill('4500000')
  const date = daysAgo(60)
  await modal.locator('#old-debt-date').fill(date)
  await expect(modal.getByTestId('old-debt-preview')).toContainText('просрочен на 47 дн.')
  await modal.getByRole('button', { name: 'Занести и ещё' }).click()

  // Окно осталось: дата та же, клиент и сумма очищены.
  await expect(modal.locator('#old-debt-client')).toBeVisible()
  await expect(modal.locator('#old-debt-sum')).toHaveValue('')
  await expect(modal.locator('#old-debt-date')).toHaveValue(date)

  // Второй долг той же фирме с той же датой и суммой — повтор спрашивает подтверждение.
  await modal.locator('#old-debt-client').fill(firm)
  await modal.getByRole('button', { name: new RegExp(firm) }).click()
  await modal.locator('#old-debt-sum').fill('4500000')
  await modal.getByRole('button', { name: 'Занести', exact: true }).click()
  await expect(modal.getByTestId('old-debt-duplicate')).toContainText('Такой долг уже занесён')
  await modal.getByRole('button', { name: 'Не заносить' }).click()
  await modal.getByRole('button', { name: 'Отмена' }).click()
  await expect(modal).toBeHidden()

  // Строка в «Должны нам»: долг и просрочка от даты из тетради.
  const row = page.getByRole('row', { name: new RegExp(firm) })
  await expect(row).toBeVisible()
  await expect(row).toContainText('47 дн.')

  // Карточка: запись «Старый долг (из тетради)» с датой долга.
  await row.click()
  await expect(page.getByText('Старый долг (из тетради)').first()).toBeVisible()
  const [y, m, d] = date.split('-')
  await expect(page.getByTestId('old-debt-date').first()).toContainText(`долг от ${d}.${m}.${y}`)
})
