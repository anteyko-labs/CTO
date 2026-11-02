// Задача 012: новый товар в ревизии — несколько штрихкодов сразу («+» у поля «Штрихкод»).
import { expect, test } from '@playwright/test'

const RUN = Date.now().toString().slice(-8)

test('ревизия: новый товар с четырьмя штрихкодами находится по любому', async ({ page }) => {
  await page.goto('/login')
  await page.getByLabel('Логин').fill(process.env.E2E_LOGIN ?? 'owner')
  await page.getByLabel('Пароль').fill(process.env.E2E_PASSWORD ?? 'owner-pass-1')
  await page.getByRole('button', { name: 'Войти' }).click()
  await expect(page.getByRole('link', { name: 'Касса' }).first()).toBeVisible()
  const H = { 'X-Device-Id': '00000000-0000-4000-8000-0000000c0de2' }
  const catName = `Коды ${RUN}`
  await page.request.post('/api/v1/categories', { data: { name: catName, kind: 'other' }, headers: H })
  await page.goto('/revision')
  await page.getByLabel('Что пересчитываем').selectOption({ label: catName })
  await page.getByRole('button', { name: 'Начать ревизию' }).click()
  await page.getByRole('button', { name: '+ Товар не из программы' }).click()
  const dialog = page.getByRole('dialog', { name: 'Товар, которого нет в программе' })
  const name = `Хомут два кода ${RUN}`
  await dialog.getByPlaceholder('Название, артикул или штрихкод').fill(name)
  await dialog.getByRole('button', { name: `+ Завести товар «${name}»` }).click()
  // Кодов сколько угодно: основной и ещё три через «+», одно пустое поле убрали.
  const codes = [1, 2, 3, 4].map((i) => `21${RUN}${i}`)
  await dialog.locator('#revnew-barcode').fill(codes[0])
  for (const code of codes.slice(1)) {
    await dialog.getByTestId('revnew-barcode-add').click()
    await dialog.getByTestId('revnew-barcode-more').last().fill(code)
  }
  await dialog.getByTestId('revnew-barcode-add').click()
  await dialog.getByRole('button', { name: 'Убрать код' }).last().click()
  await expect(dialog.getByTestId('revnew-barcode-more')).toHaveCount(3)
  await dialog.locator('#revnew-price').fill('150')
  await dialog.locator('#revnew-count').fill('3')
  await dialog.getByRole('button', { name: 'Завести и посчитать' }).click()
  await expect(dialog).toBeHidden()
  for (const code of codes) {
    const p = (await (await page.request.get(`/api/v1/products/by-barcode/${code}`)).json()) as { name: string; barcodes: string[] }
    expect(p.name).toBe(name)
    expect([...p.barcodes].sort()).toEqual(codes)
  }
  const list = (await (await page.request.get('/api/v1/revisions')).json()) as { id: string; status: string }[]
  for (const r of list.filter((x) => x.status === 'draft')) {
    const res = await page.request.post(`/api/v1/revisions/${r.id}/cancel`, { data: {}, headers: H })
    expect(res.ok(), await res.text()).toBeTruthy()
  }
})
