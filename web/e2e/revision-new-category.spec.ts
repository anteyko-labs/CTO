// Задача 007: в окне «Товар, которого нет в программе» ревизии — новая категория и вязкость масла
// из названия; у сканера — кнопка камеры.
import { expect, test, type Page } from '@playwright/test'

/** Заведён без своего кода — окно с выданным кодом (задача 007): «Готово». */
async function doneMade(page: import('@playwright/test').Page) {
  const made = page.getByTestId('revnew-made')
  if (await made.waitFor({ state: 'visible', timeout: 3_000 }).then(() => true, () => false)) await made.getByRole('button', { name: 'Готово' }).click()
}

const RUN = Date.now().toString().slice(-8)

async function login(page: Page) {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(process.env.E2E_LOGIN ?? 'owner')
  await page.getByLabel('Пароль').fill(process.env.E2E_PASSWORD ?? 'owner-pass-1')
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' }).first()).toBeVisible()
}

test('ревизия: новая категория из окна товара, вязкость масла из названия, кнопка камеры', async ({ page }) => {
  await login(page)
  await page.goto('/revision')
  await page.getByRole('button', { name: 'Начать ревизию' }).click()
  await expect(page.getByTestId('camera-scan')).toBeVisible()
  await page.getByRole('button', { name: '+ Товар не из программы' }).click()
  const dialog = page.getByRole('dialog', { name: 'Товар, которого нет в программе' })
  const name = `MOBIL SUPER 5W30 4L ${RUN}`
  await dialog.getByPlaceholder('Название, артикул или штрихкод').fill(name)
  await dialog.getByRole('button', { name: `+ Завести товар «${name}»` }).click()
  // Категории нет — создаём здесь же, она сразу выбрана.
  const catName = `Масла новые ${RUN}`
  await dialog.getByTestId('revnew-new-category').click()
  await dialog.locator('#revnew-category-name').fill(catName)
  await dialog.getByTestId('revnew-category-form').locator('select').selectOption('oil')
  await dialog.getByRole('button', { name: 'Создать категорию' }).click()
  await expect(dialog.locator('#revnew-category option:checked')).toHaveText(catName)
  // Вязкость — из названия.
  await expect(dialog.getByTestId('revnew-viscosity')).toHaveValue('5W-30')
  await dialog.locator('#revnew-liters').fill('4')
  // Цена продажи — за литр: 800 с × 4 л = канистра 3 200 с.
  await dialog.getByTestId('revnew-price-per').selectOption('l')
  await dialog.locator('#revnew-price').fill('800')
  await expect(dialog.getByText(/канистра: 3\s200/)).toBeVisible()
  await dialog.locator('#revnew-cost').fill('2500')
  await dialog.locator('#revnew-count').fill('2')
  await dialog.getByRole('button', { name: 'Завести и посчитать' }).click()
  // Своего кода нет — программа выдала код, окно показывает его и предлагает этикетку.
  await expect(page.getByTestId('revnew-made')).toContainText('программа выдала штрихкод')
  await expect(page.getByRole('button', { name: 'Напечатать этикетку' })).toBeVisible()
  await doneMade(page)
  await expect(dialog).toBeHidden()
  const found = (await (await page.request.get(`/api/v1/products?q=${encodeURIComponent(RUN)}`)).json()) as { name: string; attrs: Record<string, string> }[]
  expect(found.find((p) => p.name === name)?.attrs.viscosity).toBe('5W-30')
  expect((found.find((p) => p.name === name) as unknown as { sale_price_tyiyn: number }).sale_price_tyiyn).toBe(320_000)
  // Черновик «весь склад» не оставляем.
  const list = (await (await page.request.get('/api/v1/revisions')).json()) as { id: string; status: string }[]
  for (const r of list.filter((x) => x.status === 'draft')) await page.request.post(`/api/v1/revisions/${r.id}/cancel`, { data: {} })
})

test('ревизия: у строки — «🖨 Этикетка» с кодом товара', async ({ page }) => {
  await login(page)
  const H = { 'X-Device-Id': '00000000-0000-4000-8000-0000000c0de1' }
  const cat = (await (await page.request.post('/api/v1/categories', { data: { name: `Этикетки ${RUN}`, kind: 'other' }, headers: H })).json()) as { id: string }
  const p = (await (
    await page.request.post('/api/v1/products', { data: { op_id: crypto.randomUUID(), category_id: cat.id, name: `Шланг этикетка ${RUN}`, sale_price_tyiyn: 10_000 }, headers: H })
  ).json()) as { id: string; barcodes: string[] }
  // Без своего кода программа выдала код сама.
  expect(p.barcodes.length).toBe(1)
  await page.goto('/revision')
  await page.getByLabel('Что пересчитываем').selectOption({ label: `Этикетки ${RUN}` })
  await page.getByRole('button', { name: 'Начать ревизию' }).click()
  await page.locator('[data-view="own"]').click()
  const scanner = page.getByPlaceholder('Сканируйте штрихкод')
  await scanner.fill(p.barcodes[0])
  await scanner.press('Enter')
  await expect(page.locator(`[data-total="${p.id}"]`)).toContainText('моё 1', { timeout: 15_000 })
  await expect(page.locator(`[data-issue-code="${p.id}"]`)).toHaveText('🖨 Этикетка')
  const list = (await (await page.request.get('/api/v1/revisions')).json()) as { id: string; status: string }[]
  for (const r of list.filter((x) => x.status === 'draft')) await page.request.post(`/api/v1/revisions/${r.id}/cancel`, { data: {}, headers: H })
})
