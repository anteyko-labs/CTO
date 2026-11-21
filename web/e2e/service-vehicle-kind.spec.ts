// Услуги легковых и грузовых (задача 044, ADR-208): касса показывает услуги вида машины чека;
// выключенный в филиале вид не показывается и не предлагается. В конце — настройки как были.
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

const OWNER = process.env.E2E_LOGIN ?? 'owner'
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? 'owner-pass-1'
const DEVICE = { 'X-Device-Id': '00000000-0000-4000-8000-00000000e044' }
const RUN = Date.now().toString().slice(-8)

test.use({ viewport: { width: 1366, height: 768 } })

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

test('касса показывает услуги вида машины чека и только включённые виды', async ({ page }) => {
  await login(page)
  const request = page.request
  const before = await api<Record<string, unknown>>(request, 'GET', '/settings/cashier')
  const carName = `Мойка легковой ${RUN}`
  const truckName = `Мойка грузовой ${RUN}`
  await api(request, 'POST', '/services', { name: carName, price_tyiyn: 10_000 })
  await api(request, 'POST', '/services', { name: truckName, price_tyiyn: 20_000, vehicle_kind: 'truck' })
  const services = async () => {
    await page.getByRole('button', { name: /^Услуги/ }).first().click()
    const dlg = page.getByRole('dialog')
    await expect(dlg).toBeVisible()
    return dlg
  }
  try {
    await api(request, 'PUT', '/settings/cashier', { ...before, op_id: crypto.randomUUID(), vehicle_kind_default: 'car', services_car: true, services_truck: true })
    await page.goto('/')
    let dlg = await services()
    await expect(dlg.getByText(carName)).toBeVisible()
    await expect(dlg.getByText(truckName)).toHaveCount(0)
    // Легковая услуга в чеке, переключили на грузовую — предупреждение и «Убрать».
    await dlg.getByText(carName).click()
    await page.getByTestId('vehicle-kind-truck').click()
    const warn = page.getByTestId('wrong-kind')
    await expect(warn).toContainText('для легковых')
    await warn.getByRole('button', { name: 'Убрать' }).click()
    await expect(warn).toHaveCount(0)
    dlg = await services()
    await expect(dlg.getByText(truckName)).toBeVisible()
    await expect(dlg.getByText(carName)).toHaveCount(0)
    await page.keyboard.press('Escape')

    // Как в Айкосе: легковые выключены — только грузовая и её услуги.
    await api(request, 'PUT', '/settings/cashier', { ...before, op_id: crypto.randomUUID(), services_car: false, services_truck: true })
    await page.goto('/')
    await expect(page.getByTestId('vehicle-kind-car')).toHaveCount(0)
    await expect(page.getByTestId('vehicle-kind-truck')).toHaveAttribute('aria-checked', 'true')
    dlg = await services()
    await expect(dlg.getByText(truckName)).toBeVisible()
    await expect(dlg.getByText(carName)).toHaveCount(0)
  } finally {
    await api(request, 'PUT', '/settings/cashier', { ...before, op_id: crypto.randomUUID() })
  }
})
