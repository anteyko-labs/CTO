// Старый долг поставщику из тетради (задача 003 Н15): «Долги» → «Занести долг поставщику» →
// поставщик, сумма, дата, № накладной → строка в «Должны мы»; карточка поставщика — «Старые долги»
// с оплатой (самые старые первыми), повтор спрашивает подтверждение, акт сверки, сторно владельцем.
import { expect, test, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e315' }
const RUN = Date.now().toString().slice(-8)

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' }).first()).toBeVisible()
}

async function api<T>(page: Page, path: string, data: unknown): Promise<T> {
  const res = await page.request.fetch(`/api/v1${path}`, { method: 'POST', data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

/** Дата YYYY-MM-DD за `days` дней до сегодняшней в Бишкеке. */
function daysAgo(days: number): string {
  const today = new Date().toLocaleDateString('sv-SE', { timeZone: 'Asia/Bishkek' })
  const d = new Date(`${today}T12:00:00Z`)
  d.setUTCDate(d.getUTCDate() - days)
  return d.toISOString().slice(0, 10)
}

test('старый долг поставщику: занести, оплатить, повтор, акт сверки, сторно', async ({ page }) => {
  await login(page)
  const name = `Тетрадь Поставщик ${RUN}`
  const supplier = await api<{ id: string }>(page, '/suppliers', { name })
  const date = daysAgo(90)

  // «Долги» → «Занести долг поставщику»: поставщик, сумма, дата, номер накладной.
  await page.goto('/debts')
  await page.getByRole('button', { name: 'Занести долг поставщику' }).click()
  const modal = page.getByRole('dialog', { name: 'Старый долг поставщику' })
  await expect(modal).toBeVisible()
  await modal.locator('#supplier-debt-party').fill(name)
  await modal.getByRole('button', { name: new RegExp(name) }).click()
  await modal.locator('#supplier-debt-sum').fill('12000')
  await modal.locator('#supplier-debt-date').fill(date)
  await modal.locator('#supplier-debt-doc').fill('№ 457')
  await modal.locator('#supplier-debt-comment').fill('масло, стр. 4')
  await expect(modal.getByTestId('supplier-debt-preview')).toContainText(/станет 12\s000/)
  await modal.getByRole('button', { name: 'Занести', exact: true }).click()
  await expect(modal).toBeHidden()

  // Строка в «Должны мы».
  const row = page.getByRole('table', { name: 'Должны мы' }).getByRole('row', { name: new RegExp(name) })
  await expect(row).toContainText(/12\s000/)

  // Оплата поставщику без выбора накладной гасит старый долг.
  await api(page, '/debts/repayments', { op_id: crypto.randomUUID(), party_id: supplier.id, amount_tyiyn: 500_000, method: 'transfer' })

  // Карточка поставщика → «Старые долги»: дата, накладная, оплачено, осталось, кто занёс.
  await page.goto(`/suppliers/${supplier.id}`)
  await page.getByRole('tab', { name: 'Старые долги' }).click()
  const old = page.getByTestId('supplier-old-debt-row')
  await expect(old).toHaveCount(1)
  await expect(old).toContainText('№ 457')
  await expect(old).toContainText(/7\s000/)
  await expect(old).toContainText('масло, стр. 4')

  // Тот же долг ещё раз — «Такой долг уже занесён», второе подтверждение.
  await page.getByRole('button', { name: 'Занести старый долг' }).first().click()
  const again = page.getByRole('dialog', { name: 'Старый долг поставщику' })
  await expect(again.getByTestId('supplier-debt-party')).toContainText(name)
  await again.locator('#supplier-debt-sum').fill('12000')
  await again.locator('#supplier-debt-date').fill(date)
  await again.getByRole('button', { name: 'Занести', exact: true }).click()
  await expect(again.getByTestId('supplier-debt-duplicate')).toContainText('Такой долг уже занесён')
  await again.getByRole('button', { name: 'Не заносить' }).click()
  await again.getByRole('button', { name: 'Отмена' }).click()
  await expect(again).toBeHidden()

  // Акт сверки: строка старого долга с номером накладной.
  await page.getByRole('button', { name: 'Акт сверки' }).click()
  const act = page.getByTestId('reconciliation')
  await expect(act.getByTestId('act-row').first()).toContainText('Старый долг (из тетради) (док. поставщика № 457)')
  await page.keyboard.press('Escape')

  // Сторно владельцем: долг снят, оплата становится авансом.
  await page.goto(`/suppliers/${supplier.id}`)
  await page.getByRole('tab', { name: 'Старые долги' }).click()
  await old.getByRole('button', { name: 'Сторно' }).click()
  const rev = page.getByRole('dialog', { name: /Сторно: старый долг/ })
  await rev.getByLabel('Причина').fill('ошибка в тетради')
  await rev.getByRole('button', { name: 'Провести сторно' }).click()
  await expect(rev).toBeHidden()
  await expect(page.getByText('Старых долгов поставщику нет')).toBeVisible()
  await expect(page.getByText('Аванс поставщику')).toBeVisible()
})
