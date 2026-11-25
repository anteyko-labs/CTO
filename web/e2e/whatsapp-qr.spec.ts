// WhatsApp по QR-коду (задача 052, ADR-217): подставной шлюз на 127.0.0.1:8098 (на сервере e2e —
// WA_GATE_URL=http://127.0.0.1:8098, WA_GATE_KEY=e2e-gate-key-0123456789). Подключение по QR и по
// коду, «подключён» после события шлюза, «Отключить», настройки WhatsApp.
import http from 'node:http'
import { expect, test, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const PORT = 8098
const KEY = 'e2e-gate-key-0123456789'

let server: http.Server | null = null
let state: Record<string, unknown> = { state: 'off', phone: '', qr: '', code: '', error: '' }

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' }).first()).toBeVisible()
}

test.beforeAll(async () => {
  server = http.createServer((req, res) => {
    let body = ''
    req.on('data', (c) => (body += c))
    req.on('end', () => {
      const send = (code: number, v: unknown) => {
        res.writeHead(code, { 'content-type': 'application/json' })
        res.end(JSON.stringify(v))
      }
      if (req.headers.authorization !== `Bearer ${KEY}`) return send(401, { error: 'ключ' })
      if (req.url === '/status') return send(200, state)
      if (req.url === '/connect') {
        const b = body ? (JSON.parse(body) as { phone?: string }) : {}
        state = b.phone ? { state: 'code', code: 'ABCD1234' } : { state: 'qr', qr: '2@e2e-qr-test' }
        return send(200, state)
      }
      if (req.url === '/logout') {
        state = { state: 'off' }
        return send(200, state)
      }
      if (req.url === '/send') return send(200, { ok: true, id: 'x' })
      return send(404, { error: 'нет' })
    })
  })
  await new Promise<void>((resolve) => server!.listen(PORT, '127.0.0.1', resolve))
})

test.afterAll(async () => {
  await new Promise<void>((resolve) => (server ? server.close(() => resolve()) : resolve()))
})

test('подключение WhatsApp по QR-коду и по коду, отключение', async ({ page }) => {
  state = { state: 'off' }
  await login(page)
  await page.goto('/settings/client-messages')
  const card = page.getByTestId('wa-connect')
  await expect(card.getByTestId('wa-state')).toContainText('не подключён')
  await card.getByRole('button', { name: 'Подключить' }).click()
  await expect(card.getByTestId('wa-qr')).toBeVisible()
  await expect(card).toContainText('Связанные устройства')
  for (const width of [390, 1440]) {
    if (!process.env.SHOT_DIR) break
    await page.setViewportSize({ width, height: 900 })
    await card.screenshot({ path: `${process.env.SHOT_DIR}/wa-qr-${width}.png` })
  }
  // Без камеры — код из 8 знаков по номеру.
  await card.getByRole('button', { name: 'Нет камеры? Код вместо QR' }).click()
  await card.getByLabel('Номер WhatsApp магазина').fill('0555 12 34 56')
  await card.getByRole('button', { name: 'Получить код' }).click()
  await expect(card.getByTestId('wa-code')).toHaveText('ABCD-1234')

  // Телефон подтвердил — шлюз сообщает серверу, экран показывает номер.
  state = { state: 'connected', phone: '+996555123456', since: new Date().toISOString() }
  const ev = await page.request.post('/api/v1/whatsapp/gate-event', {
    data: { type: 'state', state: 'connected', phone: '+996555123456' },
    headers: { 'x-gate-key': KEY },
  })
  expect(ev.ok()).toBeTruthy()
  await page.reload()
  await expect(page.getByTestId('wa-state')).toContainText('подключён')
  await expect(page.getByTestId('wa-state')).toContainText('+996555123456')
  for (const width of [390, 1440]) {
    if (!process.env.SHOT_DIR) break
    await page.setViewportSize({ width, height: 900 })
    await page.getByTestId('wa-connect').screenshot({ path: `${process.env.SHOT_DIR}/wa-on-${width}.png` })
    await page.getByTestId('wa-settings').screenshot({ path: `${process.env.SHOT_DIR}/wa-settings-${width}.png` })
  }
  page.once('dialog', (d) => void d.accept())
  await page.getByTestId('wa-connect').getByRole('button', { name: 'Отключить' }).click()
  await expect(page.getByTestId('wa-state')).toContainText('не подключён')
})

test('настройки WhatsApp: предел в день и «О магазине»', async ({ page }) => {
  await login(page)
  await page.goto('/settings/client-messages')
  const card = page.getByTestId('wa-settings')
  await expect(card.getByRole('checkbox', { name: /Писать клиентам в WhatsApp/ })).toBeChecked()
  await page.locator('#wa-daily-limit').fill('0')
  await expect(card.getByText('от 1 до 1000')).toBeVisible()
  await expect(card.getByRole('button', { name: 'Сохранить' })).toBeDisabled()
  await page.locator('#wa-daily-limit').fill('150')
  await page.locator('#wa-shop-info').fill('Avtodom, Ахунбаева 1, с 9:00 до 20:00')
  await card.getByRole('button', { name: 'Сохранить' }).click()
  await expect(page.getByText('Сохранено')).toBeVisible()
  await page.reload()
  await expect(page.locator('#wa-daily-limit')).toHaveValue('150')
  await page.locator('#wa-daily-limit').fill('200')
  await page.getByTestId('wa-settings').getByRole('button', { name: 'Сохранить' }).click()
  await expect(page.getByText('Сохранено')).toBeVisible()
})
