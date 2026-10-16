// Кассир чека в сквозных проверках: при открытой смене касса его не спрашивает — кассиром
// чека будет кассир смены (SPEC-27, ADR-071); без смены его выбирают.
import { expect, type Page } from '@playwright/test'

export async function pickCashier(page: Page, name: string): Promise<void> {
  const select = page.locator('#cashier-select')
  const onShift = page.getByTestId('shift-cashier')
  await expect(select.or(onShift)).toBeVisible()
  if (await select.isVisible()) await select.selectOption({ label: name })
}
