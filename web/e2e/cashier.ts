// Кассир чека в сквозных проверках: при открытой смене касса его не спрашивает — кассиром
// чека будет кассир смены (SPEC-27, ADR-071); без смены его выбирают.
// Мастер и кассир: обычный список, а в режиме кассы — кнопки (до трёх вариантов в ряд, больше —
// кнопка и окно с крупными кнопками, SPEC-36 §16).
import { expect, type Locator, type Page } from '@playwright/test'

const pageOf = (scope: Page | Locator): Page => ('page' in scope && typeof scope.page === 'function' ? scope.page() : (scope as Page))

/** Выбрать сотрудника в поле `#id`: список или кнопки режима кассы. */
export async function choosePerson(scope: Page | Locator, id: 'master-select' | 'cashier-select', name: string): Promise<void> {
  const field = scope.locator(`#${id}`)
  await expect(field).toBeVisible()
  if ((await field.evaluate((e) => e.tagName)) === 'SELECT') {
    await field.selectOption({ label: name })
    return
  }
  const inline = field.getByRole('radio', { name, exact: true })
  if ((await inline.count()) > 0) await inline.click()
  else {
    await field.click()
    const choices = pageOf(scope).getByTestId('person-choices')
    await choices.getByRole('button', { name, exact: true }).click()
    await expect(choices).toHaveCount(0)
  }
}

export const pickMaster = (scope: Page | Locator, name: string) => choosePerson(scope, 'master-select', name)

/** Выбранный сотрудник поля: значение списка или `data-value` кнопок режима кассы. */
export async function personValue(field: Locator): Promise<string> {
  return (await field.evaluate((e) => e.tagName)) === 'SELECT' ? field.inputValue() : ((await field.getAttribute('data-value')) ?? '')
}

export async function pickCashier(page: Page, name: string): Promise<void> {
  const select = page.locator('#cashier-select')
  const onShift = page.getByTestId('shift-cashier')
  await expect(select.or(onShift)).toBeVisible()
  if (await select.isVisible()) await choosePerson(page, 'cashier-select', name)
}
