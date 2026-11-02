// Станция этикеток (задача 006, ADR-177): «телефон» (360 px) шлёт этикетку «На складе» из
// списка товаров, «станция» на ПК склада забирает задание и печатает (печать подменена), на
// телефоне — «Напечатано». Станции нет — «Склад не в сети».
import { expect, test, type Browser, type BrowserContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e006' }
const RUN = Date.now().toString().slice(-8)

/** Печать через скрытый iframe: print() окна фрейма записывает HTML в window.__prints. */
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

async function open(browser: Browser, phone: boolean): Promise<{ ctx: BrowserContext; page: Page }> {
  const ctx = await browser.newContext(phone ? { viewport: { width: 360, height: 740 }, hasTouch: true } : { viewport: { width: 1280, height: 800 } })
  const page = await ctx.newPage()
  await page.addInitScript(PRINT_STUB)
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page).not.toHaveURL(/\/login/)
  return { ctx, page }
}

async function product(page: Page, name: string, code: string) {
  const cat = await page.request.post('/api/v1/categories', { data: { name: `Кат ${name}`, kind: 'other' }, headers: DEVICE })
  expect(cat.ok(), await cat.text()).toBeTruthy()
  const p = await page.request.post('/api/v1/products', {
    data: { op_id: crypto.randomUUID(), category_id: ((await cat.json()) as { id: string }).id, name, barcodes: [code], sale_price_tyiyn: 25_000 },
    headers: DEVICE,
  })
  expect(p.ok(), await p.text()).toBeTruthy()
}

/** На телефоне: товар в этикетки из меню строки, «На складе», «Печать этикеток». */
async function sendFromPhone(page: Page, name: string) {
  await page.goto('/products')
  await page.getByPlaceholder('Название, бренд, артикул, штрихкод').fill(name)
  const row = page.locator('tr', { hasText: name }).first()
  await row.getByRole('button', { name: 'Ещё действия' }).click()
  await page.getByRole('menuitem', { name: 'Печать этикетки' }).click()
  const group = page.getByRole('group', { name: 'Куда отправить этикетки' })
  // На телефоне по умолчанию — «На складе» (настройка labels.remote_default).
  await expect(group.getByRole('button', { name: 'На складе' })).toHaveAttribute('aria-pressed', 'true')
  await page.getByRole('button', { name: 'Печать этикеток' }).click()
}

test('станция не в сети — задание ждёт, телефон предупреждает', async ({ browser }) => {
  const phone = await open(browser, true)
  const name = `Этикетка офлайн ${RUN}`
  await product(phone.page, name, `29${RUN}01`)
  await sendFromPhone(phone.page, name)
  const plate = phone.page.getByRole('status', { name: 'Печать на складе' })
  await expect(plate).toContainText('Склад не в сети')
  // Телефон ничего не печатал сам.
  expect(await phone.page.evaluate(() => (window as unknown as { __prints: string[] }).__prints.length)).toBe(0)
  await phone.ctx.close()
})

test('телефон шлёт на склад, станция печатает, на телефоне «Напечатано»', async ({ browser }) => {
  const station = await open(browser, false)
  await station.page.goto('/label-station')
  await expect(station.page.getByText('Принтер: готов')).toBeVisible()

  const phone = await open(browser, true)
  const name = `Этикетка склад ${RUN}`
  const code = `29${RUN}02`
  await product(phone.page, name, code)
  await sendFromPhone(phone.page, name)
  const plate = phone.page.getByRole('status', { name: 'Печать на складе' })
  await expect(plate).toContainText('Напечатано (1 этикетка)', { timeout: 30_000 })

  // Станция напечатала этикетку этого товара и показала задание в списке.
  await expect
    .poll(async () => (await station.page.evaluate(() => (window as unknown as { __prints: string[] }).__prints)).some((h) => h.includes(name)))
    .toBe(true)
  await expect(station.page.getByRole('list', { name: 'Задания печати' })).toContainText(name)
  expect(await phone.page.evaluate(() => (window as unknown as { __prints: string[] }).__prints.length)).toBe(0)
  await phone.ctx.close()
  await station.ctx.close()
})
