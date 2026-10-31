// Права администратора выбирает владелец (SPEC-30 §5, ADR-097, задача 003 Н8). Список на сервере — auth::PERMISSIONS.
import { useSyncExternalStore } from 'react'
import { useUser } from './auth'
import type { Permission, User } from './types'

export interface PermissionDef {
  key: Permission
  /** Подпись флажка; та же в ответе сервера «Нет права: …». */
  label: string
  /** Пояснение под флажком. */
  hint?: string
  /** Что открывает право: пункты меню и кнопки (экран «Что можно»). */
  opens?: string
  group: (typeof PERMISSION_GROUPS)[number]
  /** Выдаётся новому администратору. */
  default: boolean
}

export const PERMISSION_GROUPS = ['Разделы', 'Касса', 'Деньги', 'Склад', 'Справочники'] as const

/** Права администратора по группам экрана «Что можно». Владельцу права не нужны: он может всё.
 *  Себестоимость, прибыль, сводка, безнал, «Исправить», сторно и начисления правом не выдаются (OWNER_ONLY). */
export const PERMISSIONS: readonly PermissionDef[] = [
  { key: 'view_sales', label: 'Чеки прошлых дней', hint: 'Без права в «Чеках» — только сегодняшние', opens: 'Чеки: выбор дня', group: 'Разделы', default: true },
  { key: 'view_debts', label: 'Долги: должники и долги поставщикам', opens: 'Меню «Долги»', group: 'Разделы', default: true },
  { key: 'view_payroll', label: 'Расчёт сотрудников', hint: 'Выплата мастеру при закрытии смены работает и без него', opens: 'Меню «Расчёт»', group: 'Разделы', default: true },
  { key: 'view_income', label: 'Поступления', hint: 'Выручка по способам оплаты, категориям и товарам', opens: 'Меню «Поступления»', group: 'Разделы', default: true },
  { key: 'view_accounts', label: 'Счета: остатки и движения', hint: 'Движения кассы смены видны и без него', opens: 'Меню «Счета»', group: 'Разделы', default: true },
  { key: 'view_suppliers', label: 'Поставщики', hint: 'Карточка поставщика: поставки и накладные', opens: 'Меню «Поставщики»', group: 'Разделы', default: true },
  { key: 'view_stock', label: 'Остатки и продаваемые', opens: 'Меню «Остатки» и «Продаваемые»', group: 'Разделы', default: true },
  { key: 'reconciliation', label: 'Акт сверки', hint: 'Клиента и поставщика; кабинет фирмы и ваш бот — без изменений', opens: 'Кнопка «Акт сверки» в карточке', group: 'Разделы', default: true },
  { key: 'sale_return', label: 'Возврат по чеку', opens: 'Кнопка «Возврат» в чеке', group: 'Касса', default: true },
  { key: 'sale_debt', label: 'Продажа в долг', opens: 'Оплата «В долг» на кассе', group: 'Касса', default: true },
  { key: 'discount', label: 'Скидка в чеке', hint: 'Цена в чеке ниже цены по каталогу', opens: 'Изменение цены строки на кассе', group: 'Касса', default: false },
  { key: 'debt_payment', label: 'Оплата долга клиента', hint: 'Принять от клиента деньги в счёт долга', opens: 'Кнопка «Оплата долга»', group: 'Касса', default: true },
  { key: 'cash_moves', label: 'Внести, изъять, перевести деньги', hint: 'Только касса смены; сейфы и счета — владелец', opens: 'Кнопки «Внести / Изъять» и «Перевести»', group: 'Деньги', default: true },
  { key: 'expenses', label: 'Расходы', opens: 'Меню «Расходы», расход с кассы', group: 'Деньги', default: true },
  { key: 'payroll_pay', label: 'Выплаты мастерам', hint: 'Кассирам выплачивает владелец', opens: 'Кнопка «Выплатить» в «Расчёте» и при закрытии смены', group: 'Деньги', default: true },
  { key: 'payroll_advance', label: 'Аванс мастерам', hint: 'Не больше заработанного за услуги; сверх — только вы', opens: 'Кнопка «Аванс»', group: 'Деньги', default: false },
  { key: 'payroll_view_all', label: 'Заработок кассиров', hint: 'Без права в «Расчёте» видны только мастера; выплачиваете кассирам вы', opens: 'Кассиры в «Расчёте»', group: 'Деньги', default: false },
  { key: 'old_debts', label: 'Старые долги из тетради', hint: 'Занести долг клиента с датой из тетради; снимает его только владелец', opens: 'Кнопка «Старый долг»', group: 'Деньги', default: false },
  {
    key: 'till_pay_fix',
    label: 'Наличные ↔ QR в открытой смене',
    hint: 'Исправить способ оплаты, пока смена открыта: перенос суммы и чеки «наличные ↔ карта / QR»; закрытую смену правите вы',
    opens: 'Кнопка «Наличные ↔ QR» в смене и «Оплата» в чеках смены',
    group: 'Деньги',
    default: false,
  },
  { key: 'receipts', label: 'Приход товара и оплата поставщику', hint: 'И накладные по фото', opens: 'Меню «Приход», оплата поставщику', group: 'Склад', default: true },
  { key: 'stock_in', label: 'Оприходование', hint: 'Товар без накладной', opens: 'Меню «Оприходование»', group: 'Склад', default: true },
  { key: 'supplier_returns', label: 'Возврат поставщику', opens: 'Меню «Возврат поставщику»', group: 'Склад', default: true },
  { key: 'defects', label: 'Отметка брака', opens: 'Кнопка «Брак» в карточке товара', group: 'Склад', default: true },
  { key: 'revision', label: 'Ревизия', hint: 'Пересчёт товара; проводит ревизию владелец', opens: 'Меню «Ревизия»', group: 'Склад', default: true },
  {
    key: 'oil_transfer',
    label: 'Перелив масла',
    hint: 'Без себестоимости; о каждом переливе вам придёт уведомление, отменяете перелив только вы',
    opens: 'Меню «Перелив»',
    group: 'Склад',
    default: false,
  },
  { key: 'battery_intake', label: 'Приём аккумуляторов', opens: 'Кнопка «Приём АКБ»', group: 'Склад', default: true },
  { key: 'acceptance', label: 'Приёмка товара (планшет)', opens: 'Меню «Приёмка»', group: 'Склад', default: true },
  { key: 'products', label: 'Товары и штрихкоды', hint: 'Заводить и править карточки', opens: 'Кнопки «Новый товар» и «Изменить», этикетки, категории', group: 'Справочники', default: true },
  { key: 'clients', label: 'Клиенты и машины', hint: 'Заводить и править клиентов, их работников и машины', opens: 'Кнопки «Новый клиент» и «Изменить»', group: 'Справочники', default: true },
  { key: 'employees', label: 'Сотрудники: заводить и править', hint: 'Мастера: имя, отключить; кассиров, телефон и PIN задаёте вы', opens: 'Кнопки «Добавить» и «Изменить» в «Сотрудниках»', group: 'Справочники', default: true },
  { key: 'services', label: 'Услуги: заводить и править', hint: 'Ставку мастера задаёте вы', opens: 'Кнопки «Добавить» и «Изменить» в «Услугах»', group: 'Справочники', default: true },
  { key: 'prices', label: 'Цены товаров и услуг', hint: 'О каждой смене цены вам придёт уведомление', opens: 'Поле «Цена» в карточке товара и услуги', group: 'Справочники', default: false },
  { key: 'products_archive', label: 'Убирать товар в архив', hint: 'Только товар без остатка', opens: 'Кнопка «В архив» в карточке товара', group: 'Справочники', default: false },
]

/** Права нового администратора. */
export const DEFAULT_PERMISSIONS: Permission[] = PERMISSIONS.filter((p) => p.default).map((p) => p.key)

/** Готовые наборы экрана «Что можно» (задача 003 Н8): только расставляют флажки, сохраняет «Сохранить». */
export const PERMISSION_PRESETS: readonly { key: string; label: string; hint: string; permissions: readonly Permission[] }[] = [
  { key: 'default', label: 'По умолчанию', hint: 'Как у нового администратора', permissions: DEFAULT_PERMISSIONS },
  {
    key: 'till',
    label: 'Только касса',
    hint: 'Продажа, возврат, долг и оплата долга, клиенты; без денег, склада и разделов',
    permissions: ['sale_return', 'sale_debt', 'debt_payment', 'clients'],
  },
  { key: 'senior', label: 'Старший администратор', hint: 'Всё, что можно выдать правом', permissions: PERMISSIONS.map((p) => p.key) },
]

/** Что правом не выдаётся никогда — серый блок «Только у владельца» (I13, I14, ADR-085, ADR-097). */
export const OWNER_ONLY: readonly { label: string; why: string }[] = [
  { label: 'Сводка, прибыль, «Прибыль и убытки»', why: 'сводные доходы — только владельцу' },
  { label: 'Себестоимость, закупочные суммы, наценка', why: 'по ним видна прибыль' },
  { label: 'Безнал: банк и сверка с банком', why: 'сводные поступления' },
  { label: 'Уведомления и Телеграм владельца', why: 'это ваша лента' },
  { label: '«Исправить» и сторно документов', why: 'проведённое меняет только владелец' },
  { label: 'Начисления, бонусы, удержания, выплаты кассирам', why: 'оплата труда' },
  { label: 'Ставка мастера, отмена перелива, цена литра', why: 'решения владельца' },
  { label: 'Пользователи и права, филиалы, настройки', why: 'управление программой' },
]

/** Набор «как у другого администратора»: только известные ключи, в порядке списка. */
export function presetOf(permissions: readonly string[]): Permission[] {
  return PERMISSIONS.filter((p) => permissions.includes(p.key)).map((p) => p.key)
}

/** Включить или выключить все права группы, остальные — как были. */
export function toggleGroup(current: readonly Permission[], group: PermissionDef['group'], on: boolean): Permission[] {
  const inGroup = new Set(PERMISSIONS.filter((p) => p.group === group).map((p) => p.key))
  const rest = current.filter((k) => !inGroup.has(k))
  return presetOf(on ? [...rest, ...inGroup] : rest)
}

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
