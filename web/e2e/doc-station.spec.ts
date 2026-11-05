// Задача 034: на кассе документ A4 уходит на станцию документов (принтер HP), а не на чековый;
// станции нет — не печатается вовсе. Другие устройства печатают у себя.
import { expect, test, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'

const PRINT_STUB = () => {
  const w = window as unknown as { __prints: string[]; __sandbox: (string | null)[] }
  w.__prints = []
  w.__sandbox = []
  const desc = Object.getOwnPropertyDescriptor(HTMLIFrameElement.prototype, 'contentWindow')
  const getter = desc?.get
  if (!getter) return
  Object.defineProperty(HTMLIFrameElement.prototype, 'contentWindow', {
    configurable: true,
    get(this: HTMLIFrameElement) {
      const frame = this
      const win = getter.call(frame) as Window | null
      if (win) {
        win.print = () => {
          w.__prints.push(win.document.documentElement.outerHTML)
          w.__sandbox.push(frame.getAttribute('sandbox'))
        }
      }
      return win
    },
  })
}

const prints = (page: Page) => page.evaluate(() => (window as unknown as { __prints?: string[] }).__prints ?? [])
const sandboxes = (page: Page) => page.evaluate(() => (window as unknown as { __sandbox?: (string | null)[] }).__sandbox ?? [])

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' }).first()).toBeVisible()
}

const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e034' }

async function api<T>(page: Page, path: string, data: unknown): Promise<T> {
  const res = await page.request.fetch(`/api/v1${path}`, { method: 'POST', data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

test('документ A4: обычное устройство — у себя; касса — только на станцию документов', async ({ page, context }) => {
  await context.addInitScript(PRINT_STUB)
  await login(page)
  const run = Date.now().toString().slice(-8)
  const party = await api<{ id: string }>(page, '/parties', { role: 'customer', kind: 'person', name: `Станция Документов ${run}`, inn: `6${run}12345` })
  await api(page, '/debts/old', { op_id: crypto.randomUUID(), party_id: party.id, amount_tyiyn: 120_000, date: '2025-03-10', comment: 'стр. 7' })
  const printAct = async () => {
    await page.goto(`/clients/${party.id}`)
    await page.getByRole('button', { name: 'Акт сверки' }).click()
    await page.getByTestId('reconciliation').getByRole('button', { name: 'Печать' }).click()
  }

  // Устройство без флага печатает у себя.
  await printAct()
  await expect.poll(async () => (await prints(page)).length).toBe(1)

  // Ярлык кассы включает станцию: станции нет — на чековый не печатается.
  await page.goto('/?docs=station')
  await printAct()
  await expect(page.getByText('документ не напечатан')).toBeVisible()
  expect(await prints(page)).toHaveLength(0)

  // Окно станции открыто — документ печатается в нём, в изоляции.
  const station = await context.newPage()
  await station.goto('/label-station?size=doc')
  await expect(station.getByText('Принтер HP: документы — готов')).toBeVisible()
  await printAct()
  await expect(page.getByText('Документ ушёл на принтер HP')).toBeVisible()
  await expect.poll(async () => (await prints(station)).length, { timeout: 15_000 }).toBe(1)
  expect((await prints(station))[0]).toContain('АКТ СВЕРКИ')
  expect(await sandboxes(station)).toEqual(['allow-same-origin allow-modals'])
  await expect(station.getByLabel('Задания печати')).toContainText('напечатано')
  expect(await prints(page)).toHaveLength(0)

  // Отчёт, который печатается самой страницей, — тоже на станцию, со стилями.
  await page.goto('/profit')
  await page.getByRole('button', { name: 'Печать', exact: true }).click()
  await expect(page.getByText('Документ ушёл на принтер HP')).toBeVisible()
  await expect.poll(async () => (await prints(station)).length, { timeout: 15_000 }).toBe(2)
  expect((await prints(station))[1]).toContain('@media print')

  await page.goto('/?docs=local')
})
