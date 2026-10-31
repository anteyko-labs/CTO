// Права администратора выбирает владелец (SPEC-30 §5, ADR-097). Список на сервере — auth::PERMISSIONS.
import { useSyncExternalStore } from 'react'
import { useUser } from './auth'
import type { Permission, User } from './types'

export interface PermissionDef {
  key: Permission
  /** Подпись флажка; та же в ответе сервера «Нет права: …». */
  label: string
  /** Пояснение под флажком. */
  hint?: string
  group: (typeof PERMISSION_GROUPS)[number]
  /** Выдаётся новому администратору. */
  default: boolean
}

export const PERMISSION_GROUPS = ['Касса', 'Деньги', 'Склад', 'Справочники'] as const

/** Права администратора по группам экрана «Что можно». Владельцу права не нужны: он может всё.
 *  Себестоимость, прибыль, цены товаров, «Исправить» и начисления правом не выдаются. */
export const PERMISSIONS: readonly PermissionDef[] = [
  { key: 'sale_return', label: 'Возврат по чеку', group: 'Касса', default: true },
  { key: 'sale_debt', label: 'Продажа в долг', group: 'Касса', default: true },
  { key: 'discount', label: 'Скидка в чеке', hint: 'Цена в чеке ниже цены по каталогу', group: 'Касса', default: false },
  { key: 'debt_payment', label: 'Оплата долга клиента', hint: 'Принять от клиента деньги в счёт долга', group: 'Касса', default: true },
  { key: 'cash_moves', label: 'Внести, изъять, перевести деньги', hint: 'Только касса смены; сейфы и счета — владелец', group: 'Деньги', default: true },
  { key: 'expenses', label: 'Расходы', group: 'Деньги', default: true },
  { key: 'payroll_pay', label: 'Выплаты мастерам', hint: 'Кассирам выплачивает владелец', group: 'Деньги', default: true },
  { key: 'payroll_advance', label: 'Аванс мастерам', hint: 'Не больше заработанного за услуги; сверх — только вы', group: 'Деньги', default: false },
  { key: 'old_debts', label: 'Старые долги из тетради', hint: 'Занести долг клиента с датой из тетради; снимает его только владелец', group: 'Деньги', default: false },
  { key: 'receipts', label: 'Приход товара и оплата поставщику', hint: 'И накладные по фото', group: 'Склад', default: true },
  { key: 'stock_in', label: 'Оприходование', hint: 'Товар без накладной', group: 'Склад', default: true },
  { key: 'supplier_returns', label: 'Возврат поставщику', group: 'Склад', default: true },
  { key: 'defects', label: 'Отметка брака', group: 'Склад', default: true },
  { key: 'revision', label: 'Ревизия', hint: 'Пересчёт товара; проводит ревизию владелец', group: 'Склад', default: true },
  {
    key: 'oil_transfer',
    label: 'Перелив масла',
    hint: 'Без себестоимости; о каждом переливе вам придёт уведомление, отменяете перелив только вы',
    group: 'Склад',
    default: false,
  },
  { key: 'battery_intake', label: 'Приём аккумуляторов', group: 'Склад', default: true },
  { key: 'acceptance', label: 'Приёмка товара (планшет)', group: 'Склад', default: true },
  { key: 'products', label: 'Товары и штрихкоды', hint: 'Заводить и править карточки; цены меняет владелец', group: 'Справочники', default: true },
  { key: 'clients', label: 'Клиенты и машины', hint: 'Заводить и править клиентов, их работников и машины', group: 'Справочники', default: true },
]

/** Права нового администратора. */
export const DEFAULT_PERMISSIONS: Permission[] = PERMISSIONS.filter((p) => p.default).map((p) => p.key)

/** Группы прав, которые владелец снимает у кассира без входа (задача 003 Н10, ADR-{B8.1}). */
export const CASHIER_GROUPS: readonly PermissionDef['group'][] = ['Касса', 'Деньги']

/** Права, которые можно снять у кассира: группы «Касса» и «Деньги». */
export const CASHIER_PERMISSIONS: readonly PermissionDef[] = PERMISSIONS.filter((p) => CASHIER_GROUPS.includes(p.group))

/** Кассир открытой смены и что владелец у него снял (`cashier_denied` смены). */
export interface ShiftCashier {
  name: string
  denied: readonly string[]
}

let shiftCashier: ShiftCashier | null = null
const listeners = new Set<() => void>()

/** Касса узнала открытую смену (или что её нет): кнопки администратора — пересечение прав входа
 *  и кассира смены. Закрытая смена ограничений не несёт. */
export function rememberShiftCashier(
  shift: { cashier_name: string; cashier_denied?: readonly string[]; closed_at?: string | null } | null | undefined,
): void {
  const next = shift && !shift.closed_at && (shift.cashier_denied ?? []).length > 0 ? { name: shift.cashier_name, denied: shift.cashier_denied ?? [] } : null
  if (JSON.stringify(next) === JSON.stringify(shiftCashier)) return
  shiftCashier = next
  listeners.forEach((l) => l())
}

/** Право снято у кассира открытой смены — его имя (для подписи «Нет права у кассира …»). */
export function cashierDenied(
  user: Pick<User, 'role'>,
  permission: Permission,
  cashier: ShiftCashier | null = shiftCashier,
): string | null {
  return user.role !== 'owner' && cashier?.denied.includes(permission) ? cashier.name : null
}

/** Может ли пользователь: владелец — всё, администратор — если владелец выдал право и не снял
 *  его у кассира открытой смены (ADR-{B8.1}). Проверяет сервер; здесь — только какие пункты меню
 *  и кнопки показать. */
export function can(
  user: Pick<User, 'role' | 'permissions'>,
  permission: Permission,
  cashier: ShiftCashier | null = shiftCashier,
): boolean {
  return user.role === 'owner' || ((user.permissions ?? []).includes(permission) && !cashierDenied(user, permission, cashier))
}

/** Проверка права текущего пользователя для экранов за входом; смена кассира меняет ответ сразу. */
export function useCan(): (permission: Permission) => boolean {
  const user = useUser()
  const cashier = useSyncExternalStore(
    (l) => {
      listeners.add(l)
      return () => listeners.delete(l)
    },
    () => shiftCashier,
  )
  return (permission) => can(user, permission, cashier)
}
