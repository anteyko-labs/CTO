// Масляная книжка на выводе (задача 003 Н5, ADR-144): чек без цели кассира — сервер ставит пробег
// + интервал, и товарный чек, и чек книжки показывают одно число; канистра и разлив одного масла —
// одной строкой «4,5 л»; «Печать книжки» — в экране чека; после «Исправить» книжка нового чека
// печатается сама; «Настройки → Масляная книжка и печать» убирает блок книжки из товарного чека.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000eb03' }
const RUN = Date.now().toString().slice(-8)

/** Печать идёт через скрытый фрейм: HTML каждого документа — в window.__prints. */
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

const prints = async (page: Page) => (await page.evaluate(() => (window as unknown as { __prints?: string[] }).__prints ?? [])).map((h) => h.replace(/&nbsp;/g, ' '))
const plain = (html: string) => html.replace(/<style[\s\S]*?<\/style>/g, '').replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ')
const lastPrint = async (page: Page, what: string) => plain((await prints(page)).filter((h) => h.includes(what)).at(-1) ?? '')

async function api<T>(request: APIRequestContext, method: 'POST' | 'PUT' | 'GET', path: string, data?: unknown): Promise<T> {
  const res = await request.fetch(`/api/v1${path}`, { method, data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

/** Чек «В сервис» через API, как его прислало бы старое устройство: без следующей замены. */
async function serviceSale(request: APIRequestContext) {
  const tag = RUN
  const safe = await ownerSafe(request)
  const oilCat = await api<{ id: string }>(request, 'POST', '/categories', { name: `Масла книжки ${tag}`, kind: 'oil' })
  const oilName = `Масло книжки ${tag} 5W-30`
  const oil = await api<{ id: string }>(request, 'POST', '/products', { op_id: crypto.randomUUID(), category_id: oilCat.id, name: oilName, container_ml: 4000, sale_price_tyiyn: 300_000 })
  await api(request, 'POST', '/receipts', { op_id: crypto.randomUUID(), paid_from: safe, lines: [{ product_id: oil.id, qty: 40_000, cost_tyiyn: 2_000_000 }] })
  const svc = await api<{ id: string; price_tyiyn: number }>(request, 'POST', '/services', { name: `Замена масла в моторе книжки ${tag}`, price_tyiyn: 20_000, master_fee_tyiyn: 3_000 })
  const mst = await api<{ id: string }>(request, 'POST', '/employees', { full_name: `Мастер книжки ${tag}`, is_master: true })
  const csh = await api<{ id: string }>(request, 'POST', '/employees', { full_name: `Кассир книжки ${tag}`, is_cashier: true, pin: '1234' })
  const party = await api<{ id: string }>(request, 'POST', '/parties', { kind: 'person', first_name: 'Азиз', last_name: `Книжка${tag}`, phone: `0702${tag.slice(-6)}` })
  const vehicle = await api<{ id: string }>(request, 'POST', `/parties/${party.id}/vehicles`, { plate: `09KG${tag.slice(-3)}BKK`, brand: 'Toyota', model: 'Camry' })
  const total = 300_000 + 37_500 + svc.price_tyiyn
  // Картой: «Исправить» такого чека не требует открытой смены.
  const sale = await api<{ id: string; number: number; vehicle_id: string }>(request, 'POST', '/sales', {
    op_id: crypto.randomUUID(),
    client_time: new Date().toISOString(),
    sale_type: 'service',
    cashier_id: csh.id,
    master_id: mst.id,
    party_id: party.id,
    vehicle_id: vehicle.id,
    mileage: 130_000,
    mileage_unit: 'km',
    lines: [
      { kind: 'container', gift: false, product_id: oil.id, qty: 1, unit_price_tyiyn: 300_000 },
      { kind: 'pour', gift: false, product_id: oil.id, qty: 500, unit_price_tyiyn: 75_000 },
      { kind: 'service', gift: false, service_id: svc.id, qty: 1, unit_price_tyiyn: svc.price_tyiyn },
    ],
    payments: [{ method: 'card', amount_tyiyn: total }],
  })
  expect(sale.vehicle_id).toBe(vehicle.id)
  return { sale, oilName }
}

test('чек без цели кассира: «Печать книжки» в экране чека, одно число и «4,5 л» одной строкой; «Исправить» печатает книжку', async ({ page }) => {
  await page.addInitScript(PRINT_STUB)
  await login(page)
  const { sale, oilName } = await serviceSale(page.request)

  await page.goto(`/sales/${sale.id}`)
  // Товарный чек: блок книжки со следующей заменой, которую поставил сервер.
  await page.getByRole('button', { name: 'Печать', exact: true }).click()
  await expect.poll(async () => (await prints(page)).some((h) => h.includes('Товарный чек'))).toBe(true)
  const receipt = await lastPrint(page, 'Товарный чек')
  expect(receipt).toContain(`Масло двигателя: ${oilName} · 4,5 л`)
  expect(receipt).toContain('Масло двигателя: на 138 000 км')
  // Чек книжки этого чека — то же число, масло одной строкой.
  await page.getByRole('button', { name: 'Печать книжки' }).click()
  await expect.poll(async () => (await prints(page)).filter((h) => h.includes('Масляная книжка') && !h.includes('Товарный чек')).length).toBe(1)
  const book = plain((await prints(page)).find((h) => h.includes('Масляная книжка') && !h.includes('Товарный чек')) ?? '')
  expect(book).toContain(`${oilName} · 4,5 л`)
  expect(book.split(`${oilName}`).length - 1).toBe(1)
  expect(book.split('Следующая замена:')[1] ?? '').toContain('Масло двигателя на 138 000 км')

  // «Исправить»: новый чек печатает книжку сам, как после обычного чека.
  await page.getByTestId('fix-button').click()
  await expect(page.getByTestId('correction-banner')).toContainText(`№ ${sale.number}`)
  await page.locator('#correction-comment').fill('проверка книжки после исправления')
  const before = (await prints(page)).length
  await page.getByRole('button', { name: 'Сохранить исправление' }).click()
  await expect(page).toHaveURL(/\/sales\/[0-9a-f-]+$/)
  await expect(page).not.toHaveURL(new RegExp(sale.id))
  await expect.poll(async () => (await prints(page)).slice(before).some((h) => h.includes('Масляная книжка') && !h.includes('Товарный чек'))).toBe(true)
  await expect(page.getByRole('button', { name: 'Печать книжки' })).toBeVisible()
})

test('«Масляная книжка и печать»: без блока книжки в товарном чеке, изменение сохраняется', async ({ page }) => {
  await page.addInitScript(PRINT_STUB)
  await login(page)
  const { sale } = await serviceSale(page.request)
  const initial = await api<Record<string, unknown>>(page.request, 'GET', '/settings/oil-book')
  try {
    await page.goto('/settings')
    await page.getByRole('link', { name: /Масляная книжка и печать/ }).click()
    const block = page.getByTestId('oil-book-receipt_block')
    await expect(block).toBeChecked()
    await block.uncheck()
    await expect(page.getByText('В товарном чеке — без блока книжки')).toBeVisible()
    await page.reload()
    await expect(page.getByTestId('oil-book-receipt_block')).not.toBeChecked()
    await expect(page.getByTestId('oil-book-auto_book')).toBeChecked()

    await page.goto(`/sales/${sale.id}`)
    await expect(page.getByRole('button', { name: 'Печать книжки' })).toBeVisible()
    await page.getByRole('button', { name: 'Печать', exact: true }).click()
    await expect.poll(async () => (await prints(page)).some((h) => h.includes('Товарный чек'))).toBe(true)
    expect(await lastPrint(page, 'Товарный чек')).not.toContain('Масляная книжка')
  } finally {
    await api(page.request, 'PUT', '/settings/oil-book', { ...initial, op_id: crypto.randomUUID() })
  }
})
