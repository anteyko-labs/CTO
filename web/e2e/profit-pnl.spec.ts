// «Прибыль и убытки» (задача 003 Н12–Н13): по умолчанию «Этот месяц», раскрытие статьи до
// документа (дата, сумма, кому, кто провёл), зарплата по работнику со списком выплат, сторно —
// переключателем, «Скачать» — .xlsx с листами. Администратору отчёта нет.
import { expect, test, type Page } from '@playwright/test'
import { ownerSafe } from './safe'
import { readBook } from './xlsx'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const RUN = Date.now().toString().slice(-7)
const headers = () => ({ 'x-device-id': '00000000-0000-4000-8000-00000000e312' })

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

test('прибыль и убытки: раскрытие до документов, сторно, скачать', async ({ page, browser }) => {
  await login(page, OWNER, OWNER_PASSWORD)
  await api(page, 'put', '/settings/profit-report', { default_period: 'month', expense_payee: 'optional', show_reversals: false, expenses_bonus_accruals: true })
  const safe = await ownerSafe(page.request)
  const articles = await api<{ id: string; name: string }[]>(page, 'get', '/expense-articles')
  const household = articles.find((a) => a.name === 'Хозтовары')!.id
  const payee = `Хозмаг ${RUN}`
  await api(page, 'post', '/expenses', {
    op_id: crypto.randomUUID(),
    article_id: household,
    amount_tyiyn: 12_300,
    account_id: safe,
    payee,
    comment: 'ведро',
  })
  const wrong = await api<{ id: string }>(page, 'post', '/expenses', {
    op_id: crypto.randomUUID(),
    article_id: household,
    amount_tyiyn: 45_600,
    account_id: safe,
    payee: `Ошибка ${RUN}`,
  })
  await api(page, 'post', `/expenses/${wrong.id}/reverse`, { op_id: crypto.randomUUID(), comment: 'ошибка' })
  // Работник: бонус («доп. доход») и выплата.
  const worker = `Рома ${RUN}`
  const emp = await api<{ id: string }>(page, 'post', '/employees', { full_name: worker, is_master: true })
  await api(page, 'post', '/payroll/accruals', { op_id: crypto.randomUUID(), employee_id: emp.id, kind: 'bonus', amount_tyiyn: 21_650, comment: 'доп доход' })
  await api(page, 'post', '/payouts', { op_id: crypto.randomUUID(), employee_id: emp.id, amount_tyiyn: 21_650, source: 'account', account_id: safe })

  // Меню «Деньги» → «Прибыль и убытки»: по умолчанию этот месяц.
  await page.getByRole('link', { name: 'Прибыль и убытки' }).click()
  await expect(page.getByRole('heading', { name: 'Прибыль и убытки' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Этот месяц' })).toHaveAttribute('aria-pressed', 'true')
  await expect(page.getByText(/^С 1 /)).toBeVisible()
  const tree = page.getByTestId('pnl-tree')
  for (const line of ['Выручка', 'Себестоимость', 'Валовая прибыль', 'Операционные расходы', 'Чистая прибыль']) {
    await expect(tree.getByRole('button', { name: new RegExp(`^.?${line}`) }).first()).toBeVisible()
  }

  // Операционные расходы → Хозтовары → расход: дата, кому, касса, кто провёл.
  await page.getByTestId('pnl-opex').getByRole('button').first().click()
  const art = page.getByTestId('pnl-opex.article.Хозтовары')
  await art.getByRole('button').first().click()
  const doc = art.getByRole('treeitem').filter({ hasText: payee })
  await expect(doc).toContainText('123')
  await expect(doc).toContainText('Сейф владельца')
  await expect(doc).toContainText('Владелец')
  // Пара «расход + сторно» скрыта, переключатель её показывает серым.
  await expect(art.getByRole('treeitem').filter({ hasText: `Ошибка ${RUN}` })).toHaveCount(0)
  await page.getByLabel('Показать сторно и исправления').check()
  await expect(art.getByRole('treeitem').filter({ hasText: `Ошибка ${RUN}` })).toHaveCount(2)
  await expect(art.getByRole('treeitem').filter({ hasText: `Ошибка ${RUN}` }).getByText('сторно', { exact: true })).toBeVisible()

  // Зарплата работникам → работник → выдано списком.
  await page.getByTestId('pnl-opex.salary').getByRole('button').first().click()
  const person = page.getByTestId(`pnl-opex.staff.${emp.id}`)
  await expect(person).toContainText(worker)
  await person.getByRole('button').first().click()
  await expect(person.getByTestId(`pnl-opex.staff.${emp.id}.bonus`)).toContainText('Бонус / доп. доход')
  const paid = person.getByTestId(`pnl-opex.staff.${emp.id}.paid`)
  await paid.getByRole('button').first().click()
  await expect(paid.getByRole('treeitem').first()).toContainText('Сейф владельца')

  // «Не входит в прибыль»: выдано работникам.
  await expect(page.getByTestId('pnl-money')).toContainText('Выдано работникам')

  // «Скачать» — .xlsx с листами; первый — «Прибыль и убытки», расход и выплата в файле.
  const download = page.waitForEvent('download')
  await page.getByRole('button', { name: 'Скачать' }).click()
  const file = await download
  expect(file.suggestedFilename()).toMatch(/^Прибыль и убытки .*\.xlsx$/)
  const path = test.info().outputPath('pnl.xlsx')
  await file.saveAs(path)
  const book = readBook(path)
  expect(book.names).toEqual(['Прибыль и убытки', 'Расходы', 'Зарплата', 'По дням', 'Товары и работы', 'Не входит в прибыль'])
  expect(book.sheets[0][0][0]).toBe('Прибыль и убытки')
  const expenseRow = book.sheets[1].find((r) => r.includes(payee))
  expect(expenseRow).toBeTruthy()
  expect(expenseRow).toContain('123.00')
  expect(book.sheets[1].some((r) => r.includes('сторно'))).toBeTruthy()
  expect(book.sheets[2].some((r) => r[0] === worker && r[1] === 'Выдано: Без вида')).toBeTruthy()

  // «Расходы» → «В Excel»: все строки отбора, «Кому» — столбцом; сторно — с «Показать сторно»,
  // отметкой (задача 003 Н12б: в списке и выплаты работникам).
  await page.goto('/expenses')
  await expect(page.getByRole('cell', { name: payee })).toBeVisible()
  await page.getByLabel('Показать сторно').check()
  await expect(page.getByRole('row').filter({ hasText: `Ошибка ${RUN}` })).toHaveCount(2)
  const listFile = page.waitForEvent('download')
  await page.getByRole('button', { name: 'В Excel' }).click()
  const saved = await listFile
  expect(saved.suggestedFilename()).toMatch(/^Расходы .*\.xlsx$/)
  const listPath = test.info().outputPath('expenses.xlsx')
  await saved.saveAs(listPath)
  const list = readBook(listPath)
  expect(list.names).toEqual(['Расходы', 'Итог по статьям'])
  const head = list.sheets[0].find((r) => r[0] === '№')
  expect(head).toEqual(['№', 'Дата', 'Что', 'Кому', 'За что', 'Касса', 'Сумма', 'Кто провёл', 'Когда', 'Отметка'])
  const line = list.sheets[0].find((r) => r[3] === payee)
  expect(line?.[6]).toBe('123.00')
  expect(list.sheets[0].filter((r) => r[3] === `Ошибка ${RUN}`).map((r) => r[9])).toEqual(['сторно', 'сторнирован'])
  expect(list.sheets[0].some((r) => r[3] === worker && r[2] === 'Выплата: Без вида')).toBeTruthy()

  // Администратору отчёта нет: ни пункта меню, ни ответа сервера.
  const admin = `pnl${RUN}`
  const password = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  await api(page, 'post', '/users', { login: admin, password, full_name: `Админ ${RUN}`, role: 'admin' })
  const ctx = await browser.newContext()
  const a = await ctx.newPage()
  await login(a, admin, password)
  await expect(a.getByRole('link', { name: 'Прибыль и убытки' })).toHaveCount(0)
  const denied = await a.request.get('/api/v1/reports/profit?detail=1')
  expect(denied.status()).toBe(403)
  await ctx.close()
})

test('настройки отчёта: поле «Кому» обязательное — расход без него не записать', async ({ page }) => {
  await login(page, OWNER, OWNER_PASSWORD)
  await api(page, 'put', '/settings/profit-report', { default_period: 'month', expense_payee: 'optional', show_reversals: false, expenses_bonus_accruals: true })
  await page.goto('/settings')
  await page.getByRole('link', { name: /Отчёт о прибыли/ }).click()
  await expect(page.getByRole('heading', { name: 'Отчёт о прибыли' })).toBeVisible()
  await page.getByTestId('profit-settings-payee').getByRole('radio', { name: /Обязательно/ }).check()
  await expect(page.getByText('«Кому»: обязательно')).toBeVisible()
  try {
    await page.goto('/expenses')
    await page.locator('#expense-article').selectOption({ label: 'Хозтовары' })
    await page.locator('#expense-sum').fill('10')
    await expect(page.getByRole('button', { name: 'Записать' })).toBeDisabled()
    await page.locator('#expense-payee').fill(`Электросети ${RUN}`)
    await expect(page.getByRole('button', { name: 'Записать' })).toBeEnabled()
  } finally {
    await api(page, 'put', '/settings/profit-report', { default_period: 'month', expense_payee: 'optional', show_reversals: false, expenses_bonus_accruals: true })
  }
})
