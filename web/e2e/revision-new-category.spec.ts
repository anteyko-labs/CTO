// Задача 007: в окне «Товар, которого нет в программе» ревизии — новая категория и вязкость масла
// из названия; у сканера — кнопка камеры.
import { expect, test, type Page } from '@playwright/test'

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
  await dialog.locator('#revnew-price').fill('3000')
  await dialog.locator('#revnew-cost').fill('2500')
  await dialog.locator('#revnew-count').fill('2')
  await dialog.getByRole('button', { name: 'Завести и посчитать' }).click()
  await expect(dialog).toBeHidden()
  const found = (await (await page.request.get(`/api/v1/products?q=${encodeURIComponent(RUN)}`)).json()) as { name: string; attrs: Record<string, string> }[]
  expect(found.find((p) => p.name === name)?.attrs.viscosity).toBe('5W-30')
  // Черновик «весь склад» не оставляем.
  const list = (await (await page.request.get('/api/v1/revisions')).json()) as { id: string; status: string }[]
  for (const r of list.filter((x) => x.status === 'draft')) await page.request.post(`/api/v1/revisions/${r.id}/cancel`, { data: {} })
})
