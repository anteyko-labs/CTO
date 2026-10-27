// Касса без сети (SPEC-09, ADR-093): перезагрузка без связи сохраняет смену и кассира, клиент и
// машина находятся по снимку, неверный пробег не теряет чек, отклонённый сервером чек виден на кассе.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { ownerSafe } from './safe'
import { pickMaster } from './cashier'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2e9' }
const RUN = Date.now().toString().slice(-8)

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function apiPost<T>(request: APIRequestContext, path: string, data: unknown): Promise<T> {
  const res = await request.post(`/api/v1${path}`, { data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

/** Кладёт в очередь устройства чек, который сервер отклонит: товара с таким id нет. */
async function queueBadReceipt(page: Page, userId: string): Promise<void> {
  await page.evaluate(async (uid) => {
    const db = await new Promise<IDBDatabase>((resolve, reject) => {
      const r = indexedDB.open('avtodom', 1)
      r.onsuccess = () => resolve(r.result)
      r.onerror = () => reject(r.error)
    })
    const op = crypto.randomUUID()
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction('outbox', 'readwrite')
      tx.objectStore('outbox').put({
        op_id: op,
        temp_no: 'Ч-TEST-0001',
        created_at: new Date(Date.now() - 60_000).toISOString(),
        status: 'pending',
        attempts: 1,
        last_error: '',
        user_id: uid,
        user_name: 'Владелец',
        body: {
          op_id: op,
          client_time: new Date(Date.now() - 60_000).toISOString(),
          sale_type: 'takeaway',
          cashier_id: crypto.randomUUID(),
          lines: [{ kind: 'piece', product_id: crypto.randomUUID(), qty: 1, unit_price_tyiyn: 12300 }],
          payments: [{ method: 'cash', amount_tyiyn: 12300 }],
        },
        receipt: {
          id: '',
          number: 'Ч-TEST-0001',
          total_tyiyn: 12300,
          lines: [{ line_no: 1, kind: 'piece', name: 'Пропавший товар', qty: 1, amount_tyiyn: 12300, unit_price_tyiyn: 12300 }],
          payments: [{ method: 'cash', amount_tyiyn: 12300 }],
        },
      })
      tx.oncomplete = () => resolve()
      tx.onerror = () => reject(tx.error)
    })
  }, userId)
}

test('после входа плашка «снимок каталога устарел» уходит, как только снимок сохранён', async ({ page }) => {
  // Снимок приходит за доли секунды: плашка не висит до следующего перечитывания (раз в минуту).
  await login(page)
  await expect(page.getByText(/снимок каталога устарел/)).toHaveCount(0, { timeout: 10_000 })
})

test('без сети: перезагрузка, клиент из снимка, пробег, отклонённый чек', async ({ page, context }) => {
  const code = `27${RUN}093`
  const product = `Фильтр снимка ${RUN}`
  const plate = `01KG${RUN.slice(-3)}OFF`

  await login(page)
  const me = (await (await page.request.get('/api/v1/auth/me')).json()) as { id: string }
  const cashier = await apiPost<{ id: string }>(page.request, '/employees', { full_name: `Кассир снимка ${RUN}`, is_cashier: true, pin: '1234' })
  await apiPost(page.request, '/employees', { full_name: `Мастер снимка ${RUN}`, is_master: true })
  const cat = await apiPost<{ id: string }>(page.request, '/categories', { name: `Снимок ${RUN}`, kind: 'filter' })
  const prod = await apiPost<{ id: string }>(page.request, '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: product,
    barcodes: [code],
    sale_price_tyiyn: 93000,
  })
  await apiPost(page.request, '/receipts', {
    op_id: crypto.randomUUID(),
    paid_from: await ownerSafe(page.request),
    lines: [{ product_id: prod.id, qty: 5, cost_tyiyn: 150000 }],
  })
  const party = await apiPost<{ id: string }>(page.request, '/parties', { kind: 'person', first_name: 'Снимок', last_name: `Клиент${RUN}` })
  // Вид замены — в снимке кассы: без сети выбирается так же (ADR-096).
  await apiPost(page.request, '/services', { name: `Вид снимка ${RUN}`, price_tyiyn: 0, master_fee_tyiyn: 3000, free_with_oil: true })
  await apiPost(page.request, `/parties/${party.id}/vehicles`, { plate })
  // Смена открыта: без связи касса не должна начать спрашивать кассира.
  let shift = (await (await page.request.get('/api/v1/shifts/current')).json()) as { cashier_name: string } | null
  if (shift === null) {
    shift = await apiPost(page.request, '/shifts/open', { op_id: crypto.randomUUID(), cashier_employee_id: cashier.id, cashier_pin: '1234' })
  }
  const cashierName = shift?.cashier_name ?? ''

  // Касса со связью: оболочка ложится в кэш, снимок и смена — на устройство.
  await page.goto('/')
  await page.waitForSelector('[data-picker]')
  await page.evaluate(async () => {
    await navigator.serviceWorker.ready
  })
  await expect(page.getByTestId('shift-cashier')).toContainText(cashierName)
  await page.waitForTimeout(2500)

  // Без сети после перезагрузки — та же смена и кассир, выбора кассира нет.
  await context.setOffline(true)
  await page.reload()
  await page.waitForSelector('[data-picker]')
  await expect(page.getByTestId('shift-cashier')).toContainText(cashierName)
  await expect(page.locator('#cashier-select')).toHaveCount(0)

  // Замена в сервисе: мастер, клиент и машина — из снимка; пробег с лишними цифрами.
  const picker = page.getByPlaceholder(/Сканируйте штрихкод/)
  await picker.fill(code)
  await picker.press('Enter')
  await expect(page.getByText(product).first()).toBeVisible()
  await page.getByRole('button', { name: 'В сервис' }).click()
  await page.getByRole('dialog', { name: 'Вид замены' }).getByPlaceholder('Найти услугу').fill(`Вид снимка ${RUN}`)
  await page.getByTestId('service-choices').getByRole('button', { name: new RegExp(`Вид снимка ${RUN}`) }).click()
  await page.getByRole('dialog', { name: 'Вид замены' }).getByRole('button', { name: 'Готово' }).click()
  await pickMaster(page, `Мастер снимка ${RUN}`)
  await page.locator('[data-client-input]').fill(plate)
  await page.getByRole('button', { name: new RegExp(`Клиент${RUN}`) }).click()
  await expect(page.locator('#mileage')).toBeVisible()
  await page.locator('#mileage').fill('99999999999')
  // Поле не даёт ввести больше 7 цифр; 9 999 999 — вне диапазона, но чек без сети не теряется.
  await expect(page.locator('#mileage')).toHaveValue('9999999')
  await page.getByRole('button', { name: 'Провести чек' }).first().click()
  await expect(page.getByText(/Чек № Ч-.* проведён/)).toBeVisible()

  // Чек, который сервер не примет, — в той же очереди.
  await queueBadReceipt(page, me.id)

  // Сеть вернулась: свой чек уходит и проводится, плохой — отклонён и виден на кассе.
  await context.setOffline(false)
  await page.evaluate(() => window.dispatchEvent(new Event('online')))
  await expect
    .poll(
      async () => {
        const res = await page.request.get('/api/v1/sales')
        const day = (await res.json()) as { sales: { total_tyiyn: number; cashier_name?: string }[] }
        return day.sales.some((s) => s.total_tyiyn === 93000)
      },
      { timeout: 20000 },
    )
    .toBe(true)
  const bar = page.getByTestId('offline-bar')
  await expect(bar).toContainText('отклонено сервером: 1', { timeout: 20000 })
  await bar.click()
  const list = page.getByTestId('rejected-list')
  await expect(list).toContainText('Ч-TEST-0001')
  await expect(list).toContainText('Пропавший товар')
  await expect(list).toContainText('Причина')
  await list.getByRole('button', { name: 'Убрать' }).click()
  await list.getByRole('button', { name: 'Точно убрать?' }).click()
  await expect(page.getByTestId('rejected-list')).toHaveCount(0)
})
