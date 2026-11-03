// Кассир чека в сквозных проверках: при открытой смене касса его не спрашивает — кассиром
// чека будет кассир смены (SPEC-27, ADR-071); без смены его выбирают. Онлайн-чек — только при
// открытой смене (задача 005 Н4): `ensureShift` открывает её, если закрыта; `pickCashier` — тоже.
// Мастер и кассир: обычный список, а в режиме кассы — кнопки (до трёх вариантов в ряд, больше —
// кнопка и окно с крупными кнопками, SPEC-36 §16).
import { expect, type APIRequestContext, type Locator, type Page } from '@playwright/test'

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

const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-0000000005f4' }

export interface OpenShift {
  id: string
  number: number
  cashier_employee_id: string
  cashier_name: string
  expected_tyiyn: number
}

interface Person {
  id: string
  full_name: string
  is_cashier: boolean
  active: boolean
}

/** Открытая смена: если закрыта — открыть (задача 005 Н4: онлайн-чек — только при открытой смене).
 *  Кассир — `cashier` (id или имя), без него — первый действующий кассир или новый «Кассир смены».
 *  Подтверждение — PIN 1234, не подошёл — паролем владельца. Уже открытая смена остаётся как есть. */
export async function ensureShift(request: APIRequestContext, cashier?: string): Promise<OpenShift> {
  const cur = await request.get('/api/v1/shifts/current', { headers: DEVICE })
  expect(cur.ok(), await cur.text()).toBeTruthy()
  const now = (await cur.json()) as OpenShift | null
  if (now) return now
  const res = await request.get('/api/v1/employees', { headers: DEVICE })
  const people = res.ok() ? ((await res.json()) as Person[]).filter((e) => e.active && e.is_cashier) : []
  let id = cashier ? people.find((e) => e.id === cashier || e.full_name === cashier)?.id : people[0]?.id
  if (!id) {
    const made = await request.post('/api/v1/employees', {
      headers: DEVICE,
      data: { full_name: cashier ?? `Кассир смены ${Date.now().toString().slice(-6)}`, is_cashier: true, pin: '1234' },
    })
    expect(made.ok(), await made.text()).toBeTruthy()
    id = ((await made.json()) as Person).id
  }
  const open = (proof: Record<string, string>) =>
    request.post('/api/v1/shifts/open', { headers: DEVICE, data: { op_id: crypto.randomUUID(), cashier_employee_id: id, ...proof } })
  let opened = await open({ cashier_pin: '1234' })
  if (!opened.ok()) opened = await open({ owner_password: OWNER_PASSWORD })
  expect(opened.ok(), await opened.text()).toBeTruthy()
  return (await opened.json()) as OpenShift
}

/** Закрыть открытую смену (пересчитано ровно «должно быть») и сдать кассу с остатком в ней:
 *  проверка кассы без смены. Без сдачи плашка «касса не сдана» осталась бы следующим проверкам. */
export async function closeShiftIfOpen(request: APIRequestContext): Promise<void> {
  const cur = await request.get('/api/v1/shifts/current', { headers: DEVICE })
  const now = (await cur.json()) as OpenShift | null
  if (!now) return
  const res = await request.post(`/api/v1/shifts/${now.id}/close`, {
    headers: DEVICE,
    data: { op_id: crypto.randomUUID(), counted_tyiyn: now.expected_tyiyn, comment: '' },
  })
  expect(res.ok(), await res.text()).toBeTruthy()
  const handover = await request.post(`/api/v1/shifts/${now.id}/handover`, {
    headers: DEVICE,
    data: { op_id: crypto.randomUUID(), amount_tyiyn: 0 },
  })
  expect(handover.ok(), await handover.text()).toBeTruthy()
}

/** Касса со связью без смены (плашка «пробить чек нельзя», задача 005 Н4): открыть смену на кассира
 *  `name` и нажать «Открыть смену» на плашке — касса перечитает смену, чек не теряется. */
export async function shiftOnTill(page: Page, name?: string): Promise<void> {
  const banner = page.getByTestId('shift-needed')
  const ready = page.locator('#cashier-select').or(page.getByTestId('shift-cashier')).or(banner)
  await ready.first().waitFor({ state: 'attached', timeout: 10_000 })
  if (!(await banner.isVisible())) return
  await ensureShift(page.request, name)
  // Касса перечитывает смену по нажатию; не успела — нажать ещё раз.
  await expect(async () => {
    if (await banner.isVisible()) await banner.getByRole('button', { name: 'Открыть смену' }).click({ timeout: 2_000 })
    await expect(banner).toBeHidden({ timeout: 2_000 })
  }).toPass({ timeout: 15_000 })
}

export async function pickCashier(page: Page, name: string): Promise<void> {
  const select = page.locator('#cashier-select')
  const onShift = page.getByTestId('shift-cashier')
  await expect(select.or(onShift)).toBeVisible()
  await shiftOnTill(page, name)
  if (await select.isVisible()) await choosePerson(page, 'cashier-select', name)
}

/** Окно «Оплата» кассы (задача 026): способы оплаты, сдача и «Провести чек» — в нём, по кнопке
 *  «Оплатить» внизу колонки. Уже открыто — ничего. */
export async function openPay(page: Page): Promise<void> {
  const dialog = page.getByRole('dialog', { name: 'Оплата' })
  if (await dialog.isVisible()) return
  await page.getByTestId('pay-open').click()
  await expect(dialog).toBeVisible()
}
