// «Тип» у товаров и кросс-номера (задача 002 П10, ADR-{B3.2}): выключатель характеристики у категории,
// подбор кросс-номеров для всей категории фильтров — сразу в карточки с пометкой «подобрано
// автоматически». Сервис разбора — подставной (здесь же, на E2E_ASSISTANT_PORT): сервер запускают с
// ASSISTANT_API_KEY и ASSISTANT_API_URL=http://127.0.0.1:<порт>. Без них подбор недоступен — проверка
// подбора пропускается.
import http from 'node:http'
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000eb02' }
const RUN = Date.now().toString().slice(-8)
const PORT = Number(process.env.E2E_ASSISTANT_PORT ?? '8096')

let service: http.Server | null = null

test.beforeAll(async () => {
  service = http.createServer((req, res) => {
    req.resume()
    req.on('end', () => {
      res.writeHead(200, { 'Content-Type': 'application/json' })
      const codes = [
        { brand: 'Toyota', code: `90915-${RUN}` },
        { brand: 'Mahle', code: `OC ${RUN}` },
      ]
      res.end(
        JSON.stringify({
          candidates: [{ content: { role: 'model', parts: [{ text: JSON.stringify({ codes }) }] }, finishReason: 'STOP' }],
          usageMetadata: { promptTokenCount: 300, candidatesTokenCount: 50 },
        }),
      )
    })
  })
  await new Promise<void>((resolve) => service!.listen(PORT, '127.0.0.1', resolve))
})

test.afterAll(async () => {
  await new Promise<void>((resolve) => (service ? service.close(() => resolve()) : resolve()))
})

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(OWNER)
  await page.getByLabel('Пароль').fill(OWNER_PASSWORD)
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' })).toBeVisible()
}

async function api<T>(request: APIRequestContext, method: 'POST' | 'GET', path: string, data?: unknown): Promise<T> {
  const res = await request.fetch(`/api/v1${path}`, { method, data, headers: DEVICE })
  expect(res.ok(), `${path}: ${await res.text()}`).toBeTruthy()
  return (await res.json()) as T
}

test('категория фильтров: «Тип» выключается, кросс-номера подбираются для всей категории', async ({ page }) => {
  await login(page)
  const cat = await api<{ id: string; by_type: boolean }>(page.request, 'POST', '/categories', { name: `Фильтры подбора ${RUN}`, kind: 'filter' })
  expect(cat.by_type).toBe(true)
  const product = await api<{ id: string }>(page.request, 'POST', '/products', {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: `Фильтр подбора ${RUN}`,
    brand: 'Mann',
    article: `W${RUN}`,
    attrs: { filter_type: 'воздушный' },
    sale_price_tyiyn: 50_000,
  })

  // «Тип» выключен — строка книжки от категории; значение у товара остаётся.
  await page.goto('/categories')
  await page.getByRole('row', { name: new RegExp(`Фильтры подбора ${RUN}`) }).getByRole('cell').first().click()
  const dlg = page.getByRole('dialog', { name: 'Категория' })
  await dlg.getByTestId('attr-enabled-filter_type').uncheck()
  await expect(dlg.getByText(/«Тип» выключен — строка книжки у всех товаров категории/)).toBeVisible()
  await dlg.getByRole('button', { name: 'Сохранить' }).click()
  await expect(dlg).toHaveCount(0)
  const cats = await api<{ id: string; by_type: boolean }[]>(page.request, 'GET', '/categories')
  expect(cats.find((c) => c.id === cat.id)?.by_type).toBe(false)
  await expect(page.getByRole('row', { name: new RegExp(`Фильтры подбора ${RUN}`) })).toContainText('Фильтр масляный')

  const st = await api<{ available: boolean; mode: string }>(page.request, 'GET', '/settings/cross-pick')
  test.skip(!st.available, 'сервис разбора не настроен на сервере (ASSISTANT_API_KEY, ASSISTANT_API_URL)')
  expect(st.mode).toBe('auto')
  await page.getByRole('button', { name: `Подобрать кросс-номера в категории Фильтры подбора ${RUN}` }).click()
  const pick = page.getByTestId('cross-pick-category')
  await pick.getByRole('button', { name: 'Подобрать' }).click()
  await expect(pick).toContainText(`записано: 90915-${RUN}, OC ${RUN}`)
  await expect(pick).toContainText('Обработано 1 из 1')
  const cross = await api<{ codes: string[]; auto: string[] }>(page.request, 'GET', `/products/${product.id}/cross`)
  expect(cross.auto.sort()).toEqual([`90915-${RUN}`, `OC ${RUN}`].sort())
  // Второй раз — товар с номерами пропускается, сервис не нужен.
  await pick.getByRole('button', { name: 'Подобрать ещё раз' }).click()
  await expect(pick).toContainText('номера уже есть — пропущен')
})
