// Enter в поиске товара кладёт товар, только если выбор однозначен (SPEC-30 §13, исправлено
// 26.10.2026: «в чеке постоянно добавляет воздушный фильтр»). Enter брал первую из нескольких
// найденных строк, а первым по названию почти всегда шёл «Фильтр воздушный …»; короткий код
// Умага («1024») искался как текст — и в чек вставал фильтр, у которого эти цифры в названии.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e352' }
const RUN = Date.now().toString().slice(-8)
// Снимки экрана для проверки раскладки: только если задан каталог.
const SHOTS = process.env.E2E_SHOTS_DIR
let seq = 0

async function api<T>(request: APIRequestContext, path: string, data: unknown): Promise<T> {
  const res = await request.post(`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

/** Штучные товары одной категории с приходом по 5 штук. */
async function goods(request: APIRequestContext, items: { name: string; code: string }[]) {
  const cat = await api<{ id: string }>(request, '/categories', { name: `Поиск Enter ${RUN}${++seq}`, kind: 'filter' })
  const safe = await ownerSafe(request)
  for (const it of items) {
    const p = await api<{ id: string }>(request, '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name: it.name, barcodes: [it.code], sale_price_tyiyn: 50_000 })
    await api(request, '/receipts', { op_id: crypto.randomUUID(), paid_from: safe, lines: [{ product_id: p.id, qty: 5, cost_tyiyn: 150_000 }] })
  }
}

async function open(page: Page, touch: boolean) {
  await page.addInitScript((t) => {
    if (t) localStorage.setItem('avtodom.cashier_mode', '1')
    else localStorage.removeItem('avtodom.cashier_mode')
  }, touch)
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.locator('[data-picker]')).toBeFocused()
}

const cart = (page: Page) => page.getByTestId('cart')
const rows = (page: Page) => cart(page).getByRole('listitem')
/** Enter обработан: либо подсказка выбрать строку, либо строка в чеке. */
const settled = (page: Page) => expect(page.getByTestId('picker-choose').or(rows(page).first())).toBeVisible()

test.describe('обычная касса', () => {
  test.use({ viewport: { width: 1366, height: 768 } })

  test('найдено несколько — Enter не кладёт первое, строку выбирают стрелками или сканом', async ({ page }) => {
    await open(page, false)
    const tag = `${RUN}${++seq}`
    const model = `Ларгус${tag}`
    const air = `Фильтр воздушный ${model}`
    const oil = `Фильтр масляный ${model}`
    const cabin = `Фильтр салонный ${model}`
    await goods(page.request, [
      { name: air, code: `47${tag}1` },
      { name: oil, code: `47${tag}2` },
      { name: cabin, code: `47${tag}3` },
    ])
    const picker = page.locator('[data-picker]')
    const found = page.getByRole('button', { name: new RegExp(`^Фильтр .+ ${model}`) })

    // Список показан, Enter — ничего в чек, список остаётся, подсказка выбрать строку.
    await picker.fill(model.toLowerCase())
    await expect(found).toHaveCount(3)
    await picker.press('Enter')
    await settled(page)
    await expect(rows(page)).toHaveCount(0)
    await expect(page.getByTestId('picker-choose')).toContainText('Найдено 3')
    await expect(found).toHaveCount(3)

    // Строка, выбранная стрелкой, — то, что выбрал кассир.
    await picker.press('ArrowDown')
    await picker.press('Enter')
    await expect(rows(page).filter({ hasText: oil })).toHaveCount(1)
    await expect(rows(page)).toHaveCount(1)

    // Enter сразу после набора, до показа списка, — тоже ничего.
    await picker.fill(model.toLowerCase())
    await picker.press('Enter')
    await expect(page.getByTestId('picker-choose').or(rows(page).nth(1))).toBeVisible()
    await expect(rows(page)).toHaveCount(1)
    await expect(page.getByTestId('picker-choose')).toContainText('Найдено 3')

    // Скан кода — однозначно: товар в чеке.
    await picker.fill('')
    await page.keyboard.type(`47${tag}3`, { delay: 5 })
    await page.keyboard.press('Enter')
    await expect(rows(page).filter({ hasText: cabin })).toHaveCount(1)
    await expect(rows(page).filter({ hasText: air })).toHaveCount(0)
  })

  test('короткий код Умага: Enter не кладёт товар, у которого эти цифры в названии', async ({ page }) => {
    await open(page, false)
    const n = Number(RUN.slice(-4))
    const short = String(70_000 + n)
    const linked = String(80_000 + n)
    const filter = `E${short}L01 ВОЗДУШКА РОБОТ`
    const filter2 = `E${linked}L02 ВОЗДУШКА РОБОТ`
    const agrinol = `АГРИНОЛ 10W40 200Л связан ${RUN}`
    await goods(page.request, [
      { name: filter, code: `21${RUN}1` },
      { name: filter2, code: `21${RUN}2` },
      // Карточка, связанная со строкой Умага: доп. код — её код.
      { name: agrinol, code: linked },
    ])
    const picker = page.locator('[data-picker]')

    // Кода нет ни у одной карточки — в чек ничего, подсказка; фильтр с цифрами в названии не встаёт.
    await picker.fill(short)
    await expect(page.getByRole('button', { name: new RegExp(`^${filter}`) })).toBeVisible()
    await picker.press('Enter')
    await expect(page.getByText(`Код ${short} не найден`).or(rows(page).first())).toBeVisible()
    await expect(rows(page)).toHaveCount(0)
    await expect(page.getByText(`Код ${short} не найден`)).toBeVisible()
    await expect(picker).toHaveValue(short)

    // Код есть у карточки — встаёт она, а не фильтр.
    await picker.fill(linked)
    await picker.press('Enter')
    await expect(rows(page).filter({ hasText: agrinol })).toHaveCount(1)
    await expect(rows(page)).toHaveCount(1)
  })
})

test.describe('режим кассы 1366×768', () => {
  test.use({ viewport: { width: 1366, height: 768 }, hasTouch: true })

  test('экранная клавиатура: «Enter ↵» не кладёт первое из найденного, строки списка — над клавиатурой', async ({ page }) => {
    await open(page, true)
    const tag = `${RUN}${++seq}`
    const model = `ларгус${tag}`
    const kinds = ['воздушный', 'масляный', 'салонный', 'топливный', 'АКПП', 'вариатора', 'гидроусилителя', 'картерных газов']
    await goods(
      page.request,
      kinds.map((k, i) => ({ name: `Фильтр ${k} ${model}`, code: `48${tag}${i}` })),
    )
    await page.getByTestId('cashier-mode-bar').getByRole('button', { name: '⌨ Клавиатура' }).tap()
    const kbd = page.getByTestId('keyboard')
    await expect(kbd).toBeVisible()
    for (const ch of model) await kbd.getByRole('button', { name: ch, exact: true }).first().tap()
    await expect(page.locator('[data-picker]')).toHaveValue(model)
    const found = page.getByRole('button', { name: new RegExp(`^Фильтр .+ ${model}`, 'i') })
    await expect(found.first()).toBeVisible()
    await kbd.getByRole('button', { name: 'Enter', exact: true }).tap()
    await settled(page)
    await expect(rows(page)).toHaveCount(0)
    await expect(page.getByTestId('picker-choose')).toBeVisible()

    // Список кончается над клавиатурой и ярлыком набранного: строки не прячутся под ними.
    const list = page.locator('ul').filter({ has: found.first() })
    const box = (await list.boundingBox())!
    for (const over of [kbd, page.getByTestId('keyboard-target')]) {
      const b = (await over.boundingBox())!
      expect(box.y + box.height).toBeLessThanOrEqual(b.y + 1)
    }
    if (SHOTS) await page.screenshot({ path: `${SHOTS}/picker-enter-choose-1366.png` })

    // Касание строки — она в чеке; поиск пуст, ярлык над клавиатурой — тоже.
    await found.filter({ hasText: 'масляный' }).tap()
    await expect(rows(page).filter({ hasText: `Фильтр масляный ${model}` })).toHaveCount(1)
    await expect(page.locator('[data-picker]')).toHaveValue('')
    await expect(page.getByTestId('keyboard-value')).toHaveText('—')
  })
})
