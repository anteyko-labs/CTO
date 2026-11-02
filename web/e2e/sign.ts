// Подпись клиента за долг (задача 008): без неё чек в долг не проводится.
import { expect, type Locator, type Page } from '@playwright/test'

/** PNG 1×1 — подпись для чеков в долг, проводимых запросом. */
export const SIGNATURE_PNG = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAAAAAA6fptVAAAACklEQVR4nGNgAAAAAgABSK+kcQAAAABJRU5ErkJggg=='

/** Расписаться мышью в поле подписи. */
export async function drawSignature(page: Page, pad: Locator) {
  const canvas = pad.getByLabel('Поле подписи')
  const box = await canvas.boundingBox()
  expect(box, 'поле подписи не на экране').toBeTruthy()
  const { x, y, width, height } = box!
  await page.mouse.move(x + width * 0.1, y + height * 0.6)
  await page.mouse.down()
  for (let i = 1; i <= 12; i++) await page.mouse.move(x + width * (0.1 + i * 0.06), y + height * (i % 2 ? 0.3 : 0.7))
  await page.mouse.up()
}

/** Касса без экрана покупателя: клиент расписывается в окне кассы, чек проводится сам. */
export async function signDebtAtTill(page: Page) {
  const wait = page.getByTestId('debt-sign-wait')
  await expect(wait).toBeVisible()
  await drawSignature(page, wait)
  await wait.getByRole('button', { name: 'Подписать и провести' }).click()
}
