// QR на сумму чека Optima Business (SPEC-31) в режиме кассы: QR крупно во всю правую колонку,
// обратный отсчёт, «Отменить»; оплата — «Оплачено N с» зелёным, чек проводится со ссылкой на QR;
// после перезагрузки страницы оплаченный QR касса предлагает привязать к чеку (ADR-130).
// Банк — подставной (здесь же, на E2E_OPTIMA_PORT): сервер запускают с OPTIMA_API_KEY и
// OPTIMA_API_URL=http://127.0.0.1:<порт>. Без них QR выключен — проверка пропускается.
import http from 'node:http'
import zlib from 'node:zlib'
import { expect, test, type APIRequestContext, type Locator, type Page } from '@playwright/test'
import { ownerSafe } from './safe'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e31a' }
const RUN = Date.now().toString().slice(-8)
const PORT = Number(process.env.E2E_OPTIMA_PORT ?? '8097')
const SHOTS = process.env.E2E_SHOTS_DIR
const PICKER = /Сканируйте штрихкод/

test.use({ viewport: { width: 1366, height: 768 }, hasTouch: true })

// ---------- Картинка QR: PNG 400×400 с узором, похожим на QR ----------

const CRC = Array.from({ length: 256 }, (_, n) => {
  let c = n
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
  return c >>> 0
})
function crc32(buf: Buffer): number {
  let c = 0xffffffff
  for (const b of buf) c = CRC[(c ^ b) & 0xff] ^ (c >>> 8)
  return (c ^ 0xffffffff) >>> 0
}
function chunk(type: string, data: Buffer): Buffer {
  const len = Buffer.alloc(4)
  len.writeUInt32BE(data.length)
  const td = Buffer.concat([Buffer.from(type, 'ascii'), data])
  const crc = Buffer.alloc(4)
  crc.writeUInt32BE(crc32(td))
  return Buffer.concat([len, td, crc])
}
function qrPng(): string {
  const modules = 29
  const scale = 13
  const size = modules * scale + 23
  const dark = (x: number, y: number): boolean => {
    const finder = (fx: number, fy: number) => {
      const dx = x - fx
      const dy = y - fy
      if (dx < 0 || dy < 0 || dx > 6 || dy > 6) return null
      return dx === 0 || dy === 0 || dx === 6 || dy === 6 || (dx >= 2 && dx <= 4 && dy >= 2 && dy <= 4)
    }
    const f = finder(0, 0) ?? finder(modules - 7, 0) ?? finder(0, modules - 7)
    if (f !== null) return f
    return ((x * 7 + y * 13 + x * y) % 5) < 2
  }
  const rows: Buffer[] = []
  const pad = 11
  for (let y = 0; y < size; y++) {
    const row = Buffer.alloc(size + 1)
    for (let x = 0; x < size; x++) {
      const mx = Math.floor((x - pad) / scale)
      const my = Math.floor((y - pad) / scale)
      const inside = x >= pad && y >= pad && mx < modules && my < modules
      row[x + 1] = inside && dark(mx, my) ? 0 : 255
    }
    rows.push(row)
  }
  const ihdr = Buffer.alloc(13)
  ihdr.writeUInt32BE(size, 0)
  ihdr.writeUInt32BE(size, 4)
  ihdr[8] = 8
  ihdr[9] = 0
  const png = Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', zlib.deflateSync(Buffer.concat(rows))),
    chunk('IEND', Buffer.alloc(0)),
  ])
  return png.toString('base64')
}

// ---------- Подставной банк ----------

const bank = { generated: 0, paid: new Set<string>(), sums: new Map<string, string>(), lastBody: '' }
let server: http.Server | null = null

test.beforeAll(async () => {
  const image = qrPng()
  server = http.createServer((req, res) => {
    let body = ''
    req.on('data', (c: Buffer) => (body += c.toString()))
    req.on('end', () => {
      const url = req.url ?? ''
      const send = (code: number, v: string) => {
        res.writeHead(code, { 'Content-Type': 'application/json' })
        res.end(v)
      }
      if (req.method === 'POST' && url.startsWith('/api/v2/generate/qr')) {
        bank.generated += 1
        bank.lastBody = body
        const tx = `e2e${RUN}tx${bank.generated}`
        const sum = /"sum":([0-9.]+)/.exec(body)?.[1] ?? '0'
        bank.sums.set(tx, sum)
        return send(200, JSON.stringify({ qrBase64: image, qrUrl: 'https://pay.example/qr', transactionId: tx }))
      }
      const st = /^\/api\/v2\/get-qr-transaction-info\/(.+)$/.exec(url)
      if (st) {
        const tx = decodeURIComponent(st[1])
        if (!bank.paid.has(tx)) return send(404, JSON.stringify({ error: 'Operation not found by transactionId', code: '1003' }))
        return send(
          200,
          `{"note":"Оплата, чек кассы Avtodom","sum":${bank.sums.get(tx)},"status":"PROCESSED","transactionProcessedDateTime":"${new Date().toISOString()}","transactionId":"${tx}","operationId":"op-${tx}","payerClientType":""}`,
        )
      }
      if (url.startsWith('/api/v2/get-sale-point-infos/')) {
        return send(200, '[{"account":"1091820100040101","salePointInfoDtoList":[{"name":"Точка e2e","address":"","code":101,"cashDtoList":[{"name":"Касса 1","code":1001}]}]}]')
      }
      if (url.startsWith('/api/v1/get-pos-operation-statement')) return send(200, '{"posOperations":[]}')
      send(404, '{"error":"not found","code":1003}')
    })
  })
  await new Promise<void>((resolve) => server!.listen(PORT, '127.0.0.1', resolve))
})

test.afterAll(async () => {
  await new Promise<void>((resolve) => (server ? server.close(() => resolve()) : resolve()))
})

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'GET' | 'POST' | 'PUT', path: string, data?: unknown): Promise<T> {
  const res = await request.fetch(`/api/v1${path}`, { method, data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

async function size(l: Locator): Promise<{ width: number; height: number; x: number; y: number }> {
  const box = await l.boundingBox()
  expect(box, 'элемент не на экране').toBeTruthy()
  return box!
}

async function noPageScroll(page: Page): Promise<boolean> {
  return page.evaluate(() => {
    const el = document.scrollingElement ?? document.documentElement
    return el.scrollHeight <= window.innerHeight + 1 && el.scrollWidth <= window.innerWidth + 1
  })
}

const EMPTY = { legal_party_id: null, account_number: '', sale_point_code: null, sale_point_name: '', cash_code: null, cash_name: '', cash_account_id: null }

test('QR на сумму чека в режиме кассы: крупно, отмена, оплата и чек со ссылкой на QR', async ({ page }) => {
  await login(page)
  const status = await api<{ enabled: boolean }>(page.request, 'GET', '/optima/status')
  test.skip(!status.enabled, 'Optima Business не настроен на сервере (OPTIMA_API_KEY, OPTIMA_API_URL)')
  try {
    // Настройка владельцем: точки и кассы — из банка.
    await page.goto('/settings/optima')
    await page.locator('#optima-legal').fill('248')
    await page.locator('#optima-account').fill('1091820100040101')
    await page.getByRole('button', { name: 'Загрузить точки и кассы' }).click()
    await page.locator('#optima-point').selectOption({ label: 'Точка e2e' })
    await expect(page.locator('#optima-cash')).toHaveValue('1001')
    await page.getByRole('button', { name: 'Сохранить', exact: true }).click()
    await expect(page.getByText('Настройка сохранена')).toBeVisible()
    await expect(page.getByText('точка «Точка e2e», касса «Касса 1»')).toBeVisible()
    await page.getByRole('button', { name: 'Проверить связь' }).click()
    await expect(page.getByText('Связь есть: точка «Точка e2e»')).toBeVisible()
    if (SHOTS) await page.screenshot({ path: `${SHOTS}/optima-settings.png`, fullPage: true })

    const cat = await api<{ id: string }>(page.request, 'POST', '/categories', { name: `QR ${RUN}`, kind: 'other' })
    const name = `Освежитель QR ${RUN}`
    const p = await api<{ id: string }>(page.request, 'POST', '/products', { op_id: crypto.randomUUID(), category_id: cat.id, name, sale_price_tyiyn: 50_000 })
    await api(page.request, 'POST', '/receipts', {
      op_id: crypto.randomUUID(),
      paid_from: await ownerSafe(page.request),
      lines: [{ product_id: p.id, qty: 10, cost_tyiyn: 300_000 }],
    })
    const cashier = `Кассир QR ${RUN}`
    const emp = await api<{ id: string }>(page.request, 'POST', '/employees', { full_name: cashier, is_cashier: true, pin: '1357' })
    const cur = await api<{ id: string; expected_tyiyn: number } | null>(page.request, 'GET', '/shifts/current')
    if (cur) {
      await api(page.request, 'POST', `/shifts/${cur.id}/close`, { op_id: crypto.randomUUID(), counted_tyiyn: Math.max(cur.expected_tyiyn, 0), comment: 'сквозной тест' })
      await api(page.request, 'POST', `/shifts/${cur.id}/handover`, { op_id: crypto.randomUUID(), amount_tyiyn: 0 })
    }
    await api(page.request, 'POST', '/shifts/open', { op_id: crypto.randomUUID(), cashier_employee_id: emp.id, cashier_pin: '1357' })

    await page.goto('/')
    await page.getByRole('button', { name: 'Режим кассы', exact: true }).tap()
    await expect(page.getByTestId('cashier-mode-bar')).toBeVisible()
    await page.getByPlaceholder(PICKER).fill(name)
    await page.getByRole('button', { name: new RegExp(`^${name}`) }).first().tap()
    // Наличными кнопки QR нет; «Карта / QR банка» — есть, на сумму к оплате.
    await expect(page.getByTestId('bank-qr-create')).toHaveCount(0)
    await page.getByRole('button', { name: 'Карта / QR банка' }).tap()
    const create = page.getByTestId('bank-qr-create')
    await expect(create).toContainText(/500\sс/)
    expect((await size(create)).height).toBeGreaterThanOrEqual(56)
    expect(await noPageScroll(page)).toBe(true)
    if (SHOTS) await page.screenshot({ path: `${SHOTS}/kassa-1366-qr-button.png` })

    // Не оплатили — «Отменить»: QR больше не ждём.
    await create.tap()
    const panel = page.getByTestId('bank-qr')
    await expect(panel.getByRole('img')).toBeVisible()
    await expect(panel.getByTestId('bank-qr-wait')).toContainText(/осталось \d+:\d\d/)
    await panel.getByRole('button', { name: 'Отменить' }).tap()
    await expect(panel).toHaveCount(0)
    expect(bank.generated).toBe(1)
    expect(bank.lastBody).toContain('"sum":500.00')

    // Второй QR: крупно во всю правую колонку, страница не прокручивается, «Провести» ждёт оплату.
    const created = page.waitForResponse((r) => r.url().endsWith('/api/v1/bank-qr') && r.request().method() === 'POST')
    await create.tap()
    const qr = (await (await created).json()) as { id: string }
    await expect(panel.getByRole('img')).toBeVisible()
    const col = await size(page.getByTestId('pay-column'))
    const box = await size(panel)
    expect(box.width).toBeGreaterThanOrEqual(col.width - 2)
    expect(box.height).toBeGreaterThanOrEqual(col.height - 2)
    const img = await size(panel.getByRole('img'))
    expect(Math.min(img.width, img.height)).toBeGreaterThanOrEqual(300)
    expect((await size(panel.getByRole('button', { name: 'Отменить' }))).height).toBeGreaterThanOrEqual(56)
    expect(await noPageScroll(page)).toBe(true)
    await expect(panel.getByTestId('bank-qr-wait')).toContainText('Ждём оплату')
    await expect(page.getByRole('button', { name: 'Провести чек' })).toBeDisabled()
    if (SHOTS) await page.screenshot({ path: `${SHOTS}/kassa-1366-qr.png` })

    // Покупатель оплатил: сервер узнаёт у банка, касса — у сервера.
    bank.paid.add(`e2e${RUN}tx2`)
    const paid = page.getByTestId('bank-qr-paid')
    await expect(paid).toContainText(/Оплачено 500\sс/, { timeout: 15_000 })
    await expect(panel).toHaveCount(0)
    expect(await noPageScroll(page)).toBe(true)
    if (SHOTS) await page.screenshot({ path: `${SHOTS}/kassa-1366-qr-paid.png` })
    await page.getByRole('button', { name: 'Провести чек' }).tap()
    await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()
    const after = await api<{ status: string; sale_id: string | null }>(page.request, 'GET', `/bank-qr/${qr.id}`)
    expect(after.status).toBe('paid')
    expect(after.sale_id).toBeTruthy()

    // Оплата по QR не теряется после перезагрузки страницы (ADR-130): касса сама предлагает
    // привязать её к чеку.
    await page.getByPlaceholder(PICKER).fill(name)
    await page.getByRole('button', { name: new RegExp(`^${name}`) }).first().tap()
    await page.getByRole('button', { name: 'Карта / QR банка' }).tap()
    await create.tap()
    await expect(panel.getByRole('img')).toBeVisible()
    bank.paid.add(`e2e${RUN}tx3`)
    await expect(paid).toContainText(/Оплачено 500\sс/, { timeout: 15_000 })
    await page.reload()
    const stray = page.getByTestId('bank-qr-stray')
    await expect(stray).toContainText(/Оплачен QR на 500\sс — привязать к чеку/, { timeout: 10_000 })
    if (SHOTS) await page.screenshot({ path: `${SHOTS}/kassa-1366-qr-stray.png` })
    await stray.tap()
    await expect(paid).toContainText(/Оплачено 500\sс/)
    await expect(stray).toHaveCount(0)
    await page.getByRole('button', { name: 'Провести чек' }).tap()
    await expect(page.getByText(/Чек № \d+ проведён/)).toBeVisible()
    expect(bank.generated).toBe(3)

    await page.getByRole('button', { name: 'Выйти из режима кассы' }).tap()
    await expect(page.getByTestId('cashier-mode-bar')).toHaveCount(0)
  } finally {
    // Остальные проверки кассы — без QR.
    await api(page.request, 'PUT', '/settings/optima', EMPTY)
  }
})

test('сверка с банком: экран по дням, без ключа не видно', async ({ page }) => {
  await login(page)
  const status = await api<{ enabled: boolean }>(page.request, 'GET', '/optima/status')
  test.skip(!status.enabled, 'Optima Business не настроен на сервере')
  try {
    await api(page.request, 'PUT', '/settings/optima', { ...EMPTY, account_number: '1091820100040101' })
    await page.goto('/bank')
    await page.getByTestId('bank-recon-link').click()
    await expect(page.getByRole('heading', { name: 'Сверка с банком' })).toBeVisible()
    await page.getByRole('button', { name: 'Сверить с банком' }).click()
    await expect(page.getByText(/Выписка: операций 0, новых 0/)).toBeVisible()
    await expect(page.getByTestId('recon-day').first()).toBeVisible()
    if (SHOTS) await page.screenshot({ path: `${SHOTS}/bank-recon.png`, fullPage: true })
  } finally {
    await api(page.request, 'PUT', '/settings/optima', EMPTY)
  }
})
