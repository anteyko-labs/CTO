// «Расходы» — все траты, кроме оплаты поставщикам (задача 003 Н12б): расходы по статьям и выплаты
// работникам одним списком (дата — дата выплаты), начисленный бонус — отдельной строкой, отбор
// «Всё / Расходы / Выплаты работникам», итог с «Зарплатой работникам», «Выплатить работнику» и
// «Бонус» — окна «Расчёта», сторно — переключателем, «В Excel». Администратор без «Расчёта»
// выплат не видит.
import { expect, test, type Page } from '@playwright/test'
import { ownerSafe } from './safe'
import { readBook } from './xlsx'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const RUN = Date.now().toString().slice(-7)
const headers = () => ({ 'x-device-id': '00000000-0000-4000-8000-00000000e412' })

async function login(page: Page, user: string, password: string) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(user)
  await page.getByLabel('Пароль').fill(password)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function api<T>(page: Page, method: 'post' | 'put' | 'get', path: string, data?: unknown): Promise<T> {
  const res = await page.request[method](`/api/v1${path}`, { data, headers: headers() })
  expect(res.ok(), `${method} ${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

const SETTINGS = { default_period: 'month', expense_payee: 'optional', show_reversals: false, expenses_bonus_accruals: true }

/** «30.10.2026» — как `formatDate`. */
const ru = (iso: string) => `${iso.slice(8, 10)}.${iso.slice(5, 7)}.${iso.slice(0, 4)}`

test('расходы: выплаты и бонусы в одном списке, отбор, итог, окна «Расчёта», сторно, Excel', async ({ page, browser }) => {
  await login(page, OWNER, OWNER_PASSWORD)
  await api(page, 'put', '/settings/profit-report', SETTINGS)
  const safe = await ownerSafe(page.request)
  const today = new Intl.DateTimeFormat('en-CA', { timeZone: 'Asia/Bishkek' }).format(new Date())
  const yesterdayDate = new Date(`${today}T12:00:00Z`)
  yesterdayDate.setUTCDate(yesterdayDate.getUTCDate() - 1)
  const yesterday = yesterdayDate.toISOString().slice(0, 10)

  const articles = await api<{ id: string; name: string }[]>(page, 'get', '/expense-articles')
  const household = articles.find((a) => a.name === 'Хозтовары')!.id
  const payee = `Хозмаг ${RUN}`
  await api(page, 'post', '/expenses', { op_id: crypto.randomUUID(), article_id: household, amount_tyiyn: 12_300, account_id: safe, payee, comment: 'ведро' })
  // Ошибочный расход — по другой статье: «Прибыль и убытки» проверяет сторно в «Хозтоварах».
  const growth = articles.find((a) => a.name === 'На развитие')!.id
  const wrong = await api<{ id: string }>(page, 'post', '/expenses', {
    op_id: crypto.randomUUID(),
    article_id: growth,
    amount_tyiyn: 45_600,
    account_id: safe,
    payee: `Ошибка ${RUN}`,
  })
  await api(page, 'post', `/expenses/${wrong.id}/reverse`, { op_id: crypto.randomUUID(), comment: 'ошибка' })
  // Мастер: бонус 50 с, выдано бонусом 20 с вчерашней датой выплаты.
  const worker = `Нурлан ${RUN}`
  const emp = await api<{ id: string }>(page, 'post', '/employees', { full_name: worker, is_master: true })
  await api(page, 'post', '/payroll/accruals', { op_id: crypto.randomUUID(), employee_id: emp.id, kind: 'bonus', amount_tyiyn: 5_000, comment: `план ${RUN}` })
  await api(page, 'post', '/payouts', {
    op_id: crypto.randomUUID(),
    employee_id: emp.id,
    source: 'account',
    account_id: safe,
    pay_date: yesterday,
    comment: `премия ${RUN}`,
    lines: [{ purpose: 'bonus', amount_tyiyn: 2_000 }],
  })

  // «Расходы» с вчерашнего дня: расход, выплата датой выплаты и начисленный бонус.
  await page.getByRole('link', { name: 'Расходы' }).first().click()
  await expect(page.getByRole('heading', { name: 'Расходы' })).toBeVisible()
  await page.getByLabel('С', { exact: true }).fill(yesterday)
  await expect(page.getByRole('button', { name: 'Всё' })).toHaveAttribute('aria-pressed', 'true')
  const rowOf = (text: string) => page.getByRole('row').filter({ hasText: text })
  await expect(rowOf(payee)).toContainText('Хозтовары')
  const paid = rowOf(`премия ${RUN}`)
  await expect(paid).toContainText('Выплата: Бонус')
  await expect(paid).toContainText(worker)
  await expect(paid).toContainText(ru(yesterday))
  await expect(paid).toContainText('Сейф владельца')
  await expect(paid).toContainText('Владелец')
  const accrued = rowOf(`план ${RUN}`)
  await expect(accrued).toContainText('начислен бонус')
  await expect(accrued).toContainText('в итог не входит')
  // Пара «расход + сторно» скрыта; «Показать сторно» её показывает.
  await expect(rowOf(`Ошибка ${RUN}`)).toHaveCount(0)
  await page.getByLabel('Показать сторно').check()
  await expect(rowOf(`Ошибка ${RUN}`)).toHaveCount(2)
  await page.getByLabel('Показать сторно').uncheck()

  // Отбор: «Выплаты работникам» — без расходов, «Расходы» — без выплат.
  await page.getByRole('button', { name: 'Выплаты работникам' }).click()
  await expect(rowOf(payee)).toHaveCount(0)
  await expect(paid).toBeVisible()
  await page.getByRole('button', { name: 'Расходы', exact: true }).click()
  await expect(rowOf(payee)).toBeVisible()
  await expect(paid).toHaveCount(0)
  await page.getByRole('button', { name: 'Всё' }).click()

  // Итог по статьям: статья, «Зарплата работникам», «в т. ч. бонусы», «начислено, не выдано».
  const sum = page.getByTestId('spending-summary')
  await expect(sum.getByTestId('spending-line-article.Хозтовары')).toBeVisible()
  await expect(sum.getByTestId('spending-line-salary')).toContainText('Зарплата работникам')
  await expect(sum.getByTestId('spending-line-salary.bonus')).toContainText('в т. ч. бонусы')
  await expect(sum.getByTestId('spending-line-bonus.unpaid')).toContainText('начислено, не выдано')
  await expect(sum.getByTestId('spending-line-total')).toContainText('Итого')

  // «Выплатить работнику» — окно «Расчёта»: бонус к выдаче 30 с, выдаём 10 с.
  await page.getByRole('button', { name: 'Выплатить работнику' }).click()
  const dlg = page.getByRole('dialog')
  await dlg.locator('#staff-pay-employee').selectOption({ label: worker })
  await expect(dlg.getByTestId('payout-lines')).toContainText('Бонус')
  await dlg.getByLabel('Сумма: Бонус').fill('10')
  await dlg.getByRole('button', { name: 'Выплатить', exact: true }).click()
  await expect(dlg).toHaveCount(0)
  await expect(rowOf(worker).filter({ hasText: 'Выплата: Бонус' })).toHaveCount(2)

  // «Бонус» — окно начисления: строка «начислен бонус».
  await page.getByRole('button', { name: 'Бонус', exact: true }).click()
  await page.getByRole('dialog').locator('#staff-pay-employee').selectOption({ label: worker })
  await page.getByRole('dialog').getByLabel('Сумма, с').fill('15')
  await page.getByRole('dialog').getByLabel('Причина').fill(`ещё ${RUN}`)
  await page.getByRole('dialog').getByRole('button', { name: 'Записать' }).click()
  await expect(rowOf(`ещё ${RUN}`)).toContainText('начислен бонус')

  // «В Excel» — все строки отбора и лист итога.
  const download = page.waitForEvent('download')
  await page.getByRole('button', { name: 'В Excel' }).click()
  const file = await download
  expect(file.suggestedFilename()).toMatch(/^Расходы .*\.xlsx$/)
  const path = test.info().outputPath('expenses-all.xlsx')
  await file.saveAs(path)
  const book = readBook(path)
  expect(book.names).toEqual(['Расходы', 'Итог по статьям'])
  const head = book.sheets[0].find((r) => r[0] === '№')
  expect(head).toEqual(['№', 'Дата', 'Что', 'Кому', 'За что', 'Касса', 'Сумма', 'Кто провёл', 'Когда', 'Отметка'])
  const line = book.sheets[0].find((r) => r[4] === `премия ${RUN}`)
  expect(line?.[2]).toBe('Выплата: Бонус')
  expect(line?.[3]).toBe(worker)
  expect(line?.[6]).toBe('20.00')
  expect(book.sheets[0].find((r) => r[3] === payee)?.[6]).toBe('123.00')
  expect(book.sheets[0].find((r) => r[4] === `план ${RUN}`)?.[9]).toBe('в итог не входит')
  expect(book.sheets[1].some((r) => r[0] === 'Зарплата работникам')).toBeTruthy()

  // Настройка «Показывать начисленные бонусы» выключена — строки начисления нет.
  try {
    await api(page, 'put', '/settings/profit-report', { ...SETTINGS, expenses_bonus_accruals: false })
    await page.reload()
    await page.getByLabel('С', { exact: true }).fill(yesterday)
    await expect(paid).toBeVisible()
    await expect(rowOf(`план ${RUN}`)).toHaveCount(0)
  } finally {
    await api(page, 'put', '/settings/profit-report', SETTINGS)
  }

  // Администратор без «Расчёта сотрудников»: только расходы, ни отбора, ни кнопки выплаты.
  const admin = `exp${RUN}`
  const password = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  const user = await api<{ id: string; permissions: string[] }>(page, 'post', '/users', { login: admin, password, full_name: `Админ ${RUN}`, role: 'admin' })
  await api(page, 'put', `/users/${user.id}/permissions`, { op_id: crypto.randomUUID(), permissions: user.permissions.filter((p) => p !== 'view_payroll') })
  const ctx = await browser.newContext()
  const a = await ctx.newPage()
  await login(a, admin, password)
  await a.goto('/expenses')
  await expect(a.getByRole('heading', { name: 'Расходы' })).toBeVisible()
  await expect(a.getByText('Все траты')).toBeVisible()
  await expect(a.getByRole('button', { name: 'Выплаты работникам' })).toHaveCount(0)
  await expect(a.getByRole('button', { name: 'Выплатить работнику' })).toHaveCount(0)
  await expect(a.getByRole('row').filter({ hasText: worker })).toHaveCount(0)
  const denied = await a.request.get('/api/v1/expenses/all?kind=payouts')
  expect(denied.status()).toBe(403)
  await ctx.close()
})
