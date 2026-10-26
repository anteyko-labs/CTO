// Режим кассы: удалённая крестиком строка возвращается только кнопкой «Вернуть», а не вторым
// касанием того же места. «Вернуть» вставала над списком и сдвигала строки — на место крестика
// первой строки: двойное касание или следующее «удалить» возвращали удалённый фильтр («в чеке
// постоянно добавляет воздушный фильтр», 26.10.2026).
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e353' }
const RUN = Date.now().toString().slice(-8)
// Снимки экрана для проверки раскладки: только если задан каталог.
const SHOTS = process.env.E2E_SHOTS_DIR
let seq = 0

test.use({ viewport: { width: 1366, height: 768 }, hasTouch: true })

async function api<T>(request: APIRequestContext, path: string, data: unknown): Promise<T> {
  const res = await request.post(`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

async function goods(request: APIRequestContext) {
  const tag = `${RUN}${++seq}`
  const cat = await api<{ id: string }>(request, '/categories', { name: `Фильтры удаление ${tag}`, kind: 'filter' })
  const safe = await ownerSafe(request)
  const make = async (name: string, code: string) => {
    const p = await api<{ id: string }>(request, '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name, barcodes: [code], sale_price_tyiyn: 50_000 })
    await api(request, '/receipts', { op_id: crypto.randomUUID(), paid_from: safe, lines: [{ product_id: p.id, qty: 10, cost_tyiyn: 300_000 }] })
    return { name, code }
  }
  return { air: await make(`Фильтр воздушный удаление ${tag}`, `49${tag}1`), oil: await make(`Фильтр масляный удаление ${tag}`, `49${tag}2`) }
}

async function open(page: Page) {
  await page.addInitScript(() => localStorage.setItem('avtodom.cashier_mode', '1'))
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByTestId('cashier-mode-bar')).toBeVisible()
  await expect(page.locator('[data-picker]')).toBeFocused()
}

async function scan(page: Page, code: string, name: string) {
  await page.keyboard.type(code, { delay: 5 })
  await page.keyboard.press('Enter')
  await expect(page.getByTestId('cart').getByRole('listitem').filter({ hasText: name })).toHaveCount(1)
}

for (const kind of ['На вынос', 'В сервис'] as const) {
  for (const pause of [60, 300, 1000]) {
    test(`${kind}: второе касание крестика через ${pause} мс не возвращает удалённую строку`, async ({ page }) => {
      await open(page)
      const g = await goods(page.request)
      if (kind === 'В сервис') {
        await page.getByRole('button', { name: 'В сервис' }).tap()
        const kinds = page.getByRole('dialog', { name: 'Вид замены' })
        await expect(kinds).toBeVisible()
        await kinds.getByRole('button', { name: 'Закрыть' }).tap()
        await expect(kinds).toHaveCount(0)
        await page.locator('[data-picker]').focus()
      }
      await scan(page, g.air.code, g.air.name)
      await scan(page, g.oil.code, g.oil.name)
      const rows = page.getByTestId('cart').getByRole('listitem')
      await expect(rows).toHaveCount(2)
      const x = (await rows.first().getByRole('button', { name: 'Удалить строку' }).boundingBox())!
      const cx = x.x + x.width / 2
      const cy = x.y + x.height / 2

      await page.touchscreen.tap(cx, cy)
      await expect(rows.filter({ hasText: g.air.name })).toHaveCount(0)
      await expect(page.getByRole('button', { name: 'Вернуть' })).toBeVisible()
      if (SHOTS && pause === 60) await page.screenshot({ path: `${SHOTS}/cashier-undo-${kind === 'В сервис' ? 'service' : 'takeaway'}-1366.png` })
      // Что под пальцем после удаления: не должна быть «Вернуть».
      const underFinger = await page.evaluate(([px, py]) => Boolean(document.elementFromPoint(px, py)?.closest('button')?.textContent?.includes('Вернуть')), [cx, cy])

      await page.waitForTimeout(pause)
      await page.touchscreen.tap(cx, cy)
      await page.waitForTimeout(300)
      await expect(rows.filter({ hasText: g.air.name })).toHaveCount(0)
      expect(underFinger).toBe(false)

      // «Вернуть» по-прежнему возвращает последнюю удалённую строку.
      const back = page.getByRole('button', { name: 'Вернуть' })
      if (await back.isVisible()) {
        await back.tap()
        await expect(rows.filter({ hasText: pause < 400 ? g.air.name : g.oil.name })).toHaveCount(1)
      }
    })
  }
}
