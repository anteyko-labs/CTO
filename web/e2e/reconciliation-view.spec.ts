// Акт сверки на экране (задача 003 Н9): окно с периодом, сальдо и строками; печать — только по
// кнопке. Акт — по праву «Акт сверки» (Н8): без него кнопки нет, сервер отвечает «Нет права».
import { expect, test, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e309' }
const RUN = Date.now().toString().slice(-8)

const PRINT_STUB = () => {
  const w = window as unknown as { __prints: string[] }
  w.__prints = []
  const desc = Object.getOwnPropertyDescriptor(HTMLIFrameElement.prototype, 'contentWindow')
  const getter = desc?.get
  if (!getter) return
  Object.defineProperty(HTMLIFrameElement.prototype, 'contentWindow', {
    configurable: true,
    get(this: HTMLIFrameElement) {
      const win = getter.call(this) as Window | null
      if (win) {
        try {
          win.print = () => {
            w.__prints.push(win.document.documentElement.outerHTML)
          }
        } catch {
          // Окно чужого источника — не наше.
        }
      }
      return win
    },
  })
}

const prints = async (page: Page) => page.evaluate(() => (window as unknown as { __prints?: string[] }).__prints ?? [])

async function login(page: Page, user: string, password: string) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(user)
  await page.getByLabel('Пароль').fill(password)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' }).first()).toBeVisible()
}

async function api<T>(page: Page, method: 'POST' | 'PUT', path: string, data: unknown): Promise<T> {
  const res = await page.request.fetch(`/api/v1${path}`, { method, data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

test('акт сверки открывается на экране: период, сальдо, строки; печать — по кнопке', async ({ page, browser }) => {
  await page.addInitScript(PRINT_STUB)
  await login(page, OWNER, OWNER_PASSWORD)
  const name = `Сверка Экран ${RUN}`
  const party = await api<{ id: string }>(page, 'POST', '/parties', { role: 'customer', kind: 'person', name, inn: `5${RUN}12345` })
  // Долг из тетради прошлого года: в «Этом месяце» он — в сальдо на начало, не строкой.
  await api(page, 'POST', '/debts/old', { op_id: crypto.randomUUID(), party_id: party.id, amount_tyiyn: 120_000, date: '2025-03-10', comment: 'стр. 7' })

  await page.goto(`/clients/${party.id}`)
  await page.getByRole('button', { name: 'Акт сверки' }).click()
  const act = page.getByTestId('reconciliation')
  // По умолчанию — всё время: строка долга и вывод на экране, на принтер ничего не ушло.
  await expect(act.getByTestId('act-row')).toHaveCount(1)
  await expect(act.getByTestId('act-opening')).toBeVisible()
  await expect(act.getByTestId('act-closing')).toContainText(/1\s200/)
  await expect(act.getByTestId('act-verdict')).toContainText(`${name} должен`)
  expect(await prints(page)).toHaveLength(0)

  await act.getByRole('button', { name: 'Этот месяц' }).click()
  await expect(act.getByTestId('act-row')).toHaveCount(0)
  await expect(act.getByTestId('act-opening')).toContainText(/1\s200/)
  await act.getByRole('button', { name: 'Всё время' }).click()
  await expect(act.getByTestId('act-row')).toHaveCount(1)

  await act.getByRole('button', { name: 'Печать' }).click()
  await expect.poll(async () => (await prints(page)).length, { message: 'акт ушёл на печать' }).toBe(1)
  expect((await prints(page))[0]).toContain('АКТ СВЕРКИ')

  // Период по умолчанию — «Настройки → Документы о долге».
  await page.goto('/settings/documents')
  await page.getByTestId('act-default-period').selectOption('month')
  await page.getByRole('button', { name: 'Сохранить' }).click()
  await expect(page.getByText('Сохранено')).toBeVisible()
  await page.goto(`/clients/${party.id}`)
  await page.getByRole('button', { name: 'Акт сверки' }).click()
  await expect(act.getByRole('button', { name: 'Этот месяц' })).toHaveAttribute('aria-pressed', 'true')
  await expect(act.getByTestId('act-row')).toHaveCount(0)
  await page.goto('/settings/documents')
  await page.getByTestId('act-default-period').selectOption('all')
  await page.getByRole('button', { name: 'Сохранить' }).click()
  await expect(page.getByText('Сохранено')).toBeVisible()

  // Администратор без права «Акт сверки»: кнопки нет, сервер отвечает «Нет права».
  const admin = `akt${RUN}`
  const password = `pass-${RUN}-${Math.random().toString(36).slice(2)}`
  const user = await api<{ id: string; permissions: string[] }>(page, 'POST', '/users', { login: admin, password, full_name: `Акт ${RUN}`, role: 'admin' })
  await api(page, 'PUT', `/users/${user.id}/permissions`, { op_id: crypto.randomUUID(), permissions: user.permissions.filter((p) => p !== 'reconciliation') })
  const ctx = await browser.newContext()
  const a = await ctx.newPage()
  await login(a, admin, password)
  await a.goto(`/clients/${party.id}`)
  await expect(a.getByRole('heading', { name })).toBeVisible()
  await expect(a.getByRole('button', { name: 'Акт сверки' })).toHaveCount(0)
  const denied = await a.request.get(`/api/v1/parties/${party.id}/reconciliation`)
  expect(denied.status()).toBe(403)
  expect(((await denied.json()) as { error: { message: string } }).error.message).toBe('Нет права: Акт сверки')
  await ctx.close()
})
