// «Настройки → Касса» (задача 003 Н2–Н4, ADR-146): где стоит каждая кнопка режима кассы, высота
// плиток ходового, купюры у «Получено», скидка на кассе и прибыль на бумаге. Хранение — `settings`
// филиала, ключ `cashier` (`GET/PUT /settings/cashier`); без связи — из снимка кассы. Меняет только
// владелец. Права на кнопки не меняются: кнопка без права не показывается, где бы она ни стояла.
// Тот же список кнопок — `BUTTONS` в server/src/api/cashier_settings.rs (тест сервера сверяет).
import { get } from './api'
import { readSnapshot } from './offline'
import { can } from './permissions'
import type { Permission, User } from './types'

/** Место кнопки: под поиском; у «Итого»; в колонке оплаты под клиентом (как было); в окне «Ещё
 *  действия…»; скрыта; полоса сверху (только у кнопок полосы). */
export type Place = 'search' | 'total' | 'pay' | 'more' | 'hidden' | 'bar'

export const PLACE_LABELS: Record<Place, string> = {
  search: 'Под поиском',
  total: 'У «Итого»',
  pay: 'Под оплатой',
  more: 'В «Ещё действия…»',
  hidden: 'Скрыть',
  bar: 'В полосе сверху',
}

const ACTION: Place[] = ['search', 'total', 'pay', 'more', 'hidden']
const BAR: Place[] = ['bar', 'more', 'hidden']
/** «Наличные ↔ QR» (задача 003): и в полосе сверху, и среди действий. */
const PAY_FIX: Place[] = ['bar', 'search', 'total', 'pay', 'more', 'hidden']

export type ButtonId =
  | 'debt_payment'
  | 'expense'
  | 'battery_intake'
  | 'battery_sell'
  | 'staff_pay'
  | 'no_receipt'
  | 'discount'
  | 'delivery'
  | 'comment'
  | 'battery_pay'
  | 'bonus'
  | 'shift_print'
  | 'keyboard'
  | 'fullscreen'
  | 'exit'
  | 'pay_fix'

export interface ButtonDef {
  id: ButtonId
  label: string
  /** Где может стоять. */
  places: Place[]
  /** Место по умолчанию — нынешняя раскладка (SPEC-29 «Раскладка»). */
  place: Place
  /** Право вошедшего; `owner` — только владелец (возврат без чека, ADR-105). */
  right?: Permission | 'owner'
}

/** Все кнопки по порядку по умолчанию. */
export const BUTTONS: readonly ButtonDef[] = [
  { id: 'debt_payment', label: 'Принять оплату долга', places: ACTION, place: 'search', right: 'debt_payment' },
  { id: 'expense', label: 'Мелкий расход из кассы', places: ACTION, place: 'search', right: 'expenses' },
  { id: 'battery_intake', label: 'Приём аккумуляторов', places: ACTION, place: 'search', right: 'battery_intake' },
  { id: 'battery_sell', label: 'Аккумуляторы на вес', places: ACTION, place: 'search' },
  { id: 'staff_pay', label: 'Выплата сотруднику', places: ACTION, place: 'search', right: 'payroll_pay' },
  { id: 'no_receipt', label: 'Возврат без чека', places: ACTION, place: 'search', right: 'owner' },
  { id: 'delivery', label: 'Доставка', places: ACTION, place: 'pay' },
  { id: 'comment', label: 'Комментарий', places: ACTION, place: 'pay' },
  { id: 'battery_pay', label: 'Сдал аккумулятор', places: ACTION, place: 'pay' },
  { id: 'bonus', label: 'Баллы клиента', places: ACTION, place: 'pay' },
  // Новая кнопка — рядом с баллами: экран кассы по умолчанию прежний; «Частое — у итога» — к «Итого».
  { id: 'discount', label: 'Скидка', places: ACTION, place: 'pay' },
  { id: 'shift_print', label: 'Чек смены', places: BAR, place: 'bar' },
  { id: 'keyboard', label: '⌨ Клавиатура', places: BAR, place: 'bar' },
  { id: 'fullscreen', label: 'Во весь экран', places: BAR, place: 'bar' },
  { id: 'exit', label: 'Выйти из режима кассы', places: ['bar', 'more'], place: 'bar' },
  // «Наличные ↔ QR» открытой смены (задача 003 Н1): владельцу и администратору с правом; по
  // умолчанию скрыта — экран кассы прежний, окно есть и в «Закрыть смену».
  { id: 'pay_fix', label: 'Наличные ↔ QR', places: PAY_FIX, place: 'hidden', right: 'till_pay_fix' },
]

export interface ButtonPlace {
  id: ButtonId
  place: Place
}

export interface CashierSettings {
  buttons: ButtonPlace[]
  /** «Провести чек» во всю ширину, «Отложить» и «Очистить» — узким рядом над ним. */
  submit_wide: boolean
  /** «Выйти из режима кассы» — серой кнопкой. */
  exit_grey: boolean
  /** Высота плиток ходового, % высоты экрана. */
  tiles_dvh: number
  /** Купюр у «Получено» (1–3). */
  notes: number
  /** Быстрые проценты в окне «Скидка». */
  discount_quick: number[]
  /** Скидка на весь чек снижает и цены услуг. */
  discount_services: boolean
  /** Предел скидки администратора на чек: % от суммы по прайсу и сумма; пусто — без предела. */
  admin_limit_pct: number | null
  admin_limit_tyiyn: number | null
  /** Строка «Скидка» в печатном чеке. */
  print_discount: boolean
  /** Раздел «Прибыль» в чеке смены (по умолчанию нет). */
  print_shift_profit: boolean
  /** Себестоимость и прибыль на экране чека в режиме кассы (по умолчанию нет). */
  sale_cost_on_till: boolean
  /** «Продажа только при открытой смене» (задача 005 Н4): при связи без смены чек не пробивается
   *  (по умолчанию да — прямая просьба заказчика). */
  sale_requires_shift: boolean
}

export const DEFAULT_CASHIER: CashierSettings = {
  buttons: BUTTONS.map((b) => ({ id: b.id, place: b.place })),
  submit_wide: false,
  exit_grey: false,
  tiles_dvh: 24,
  notes: 1,
  discount_quick: [5, 10, 15],
  discount_services: true,
  admin_limit_pct: null,
  admin_limit_tyiyn: null,
  print_discount: false,
  print_shift_profit: false,
  sale_cost_on_till: false,
  sale_requires_shift: true,
}

export const buttonDef = (id: ButtonId): ButtonDef | undefined => BUTTONS.find((b) => b.id === id)

/** Настройка с сервера или снимка — полная: неизвестные кнопки отброшены, недостающие — на своих
 *  местах по умолчанию в конце, отсутствующие поля — по умолчанию. */
export function withDefaults(raw: Partial<CashierSettings> | null | undefined): CashierSettings {
  const s = { ...DEFAULT_CASHIER, ...(raw ?? {}) }
  const seen = new Set<string>()
  const buttons: ButtonPlace[] = []
  for (const b of raw?.buttons ?? []) {
    const def = buttonDef(b.id)
    if (!def || seen.has(b.id) || !def.places.includes(b.place)) continue
    seen.add(b.id)
    buttons.push({ id: b.id, place: b.place })
  }
  for (const b of BUTTONS) if (!seen.has(b.id)) buttons.push({ id: b.id, place: b.place })
  return { ...s, buttons }
}

/** Видна ли кнопка вошедшему: право проверяет и сервер (I12). */
export function allowed(user: Pick<User, 'role' | 'permissions'>, id: ButtonId): boolean {
  const right = buttonDef(id)?.right
  if (!right) return true
  return right === 'owner' ? user.role === 'owner' : can(user, right)
}

/** Кнопки места по порядку — только те, на которые у вошедшего есть право. */
export function placed(s: CashierSettings, place: Place, user: Pick<User, 'role' | 'permissions'>): ButtonId[] {
  return s.buttons.filter((b) => b.place === place && allowed(user, b.id)).map((b) => b.id)
}

/** Где стоит кнопка. */
export function placeOf(s: CashierSettings, id: ButtonId): Place {
  return s.buttons.find((b) => b.id === id)?.place ?? buttonDef(id)?.place ?? 'hidden'
}

/** Перенести кнопку в другое место: в конец этого места. */
export function setPlace(list: ButtonPlace[], id: ButtonId, place: Place): ButtonPlace[] {
  const def = buttonDef(id)
  if (!def || !def.places.includes(place)) return list
  return [...list.filter((b) => b.id !== id), { id, place }]
}

/** Сдвинуть кнопку на одну позицию внутри её места (−1 — выше, +1 — ниже). */
export function move(list: ButtonPlace[], id: ButtonId, dir: -1 | 1): ButtonPlace[] {
  const i = list.findIndex((b) => b.id === id)
  if (i < 0) return list
  const place = list[i].place
  let j = i + dir
  while (j >= 0 && j < list.length && list[j].place !== place) j += dir
  if (j < 0 || j >= list.length) return list
  const next = [...list]
  ;[next[i], next[j]] = [next[j], next[i]]
  return next
}

/** Готовый набор «Частое — у итога»: у «Итого» — скидка, баллы и аккумулятор в оплату; редкие
 *  действия — в окне «Ещё действия…», плитки ходового выше; «Провести чек» во всю ширину;
 *  «Выйти из режима кассы» — последней и серой. Остальное — как было; затем можно поправить. */
export function frequentPreset(s: CashierSettings): CashierSettings {
  const at: Partial<Record<ButtonId, Place>> = {
    discount: 'total',
    bonus: 'total',
    battery_pay: 'total',
    delivery: 'pay',
    comment: 'pay',
    debt_payment: 'more',
    expense: 'more',
    battery_intake: 'more',
    battery_sell: 'more',
    staff_pay: 'more',
    no_receipt: 'more',
    shift_print: 'bar',
    keyboard: 'bar',
    fullscreen: 'bar',
    exit: 'bar',
  }
  const order: ButtonId[] = ['discount', 'bonus', 'battery_pay', ...BUTTONS.map((b) => b.id).filter((id) => !['discount', 'bonus', 'battery_pay'].includes(id))]
  return {
    ...s,
    buttons: order.map((id) => ({ id, place: at[id] ?? buttonDef(id)?.place ?? 'hidden' })),
    submit_wide: true,
    exit_grey: true,
    tiles_dvh: 34,
  }
}

/** Как было: раскладка по умолчанию; скидка, предел и прибыль не трогаются. */
export function layoutReset(s: CashierSettings): CashierSettings {
  return {
    ...s,
    buttons: DEFAULT_CASHIER.buttons,
    submit_wide: DEFAULT_CASHIER.submit_wide,
    exit_grey: DEFAULT_CASHIER.exit_grey,
    tiles_dvh: DEFAULT_CASHIER.tiles_dvh,
    notes: DEFAULT_CASHIER.notes,
  }
}

/** Настройка кассы: с сервера, без связи — из снимка кассы, иначе по умолчанию. */
export async function loadCashierSettings(): Promise<CashierSettings> {
  try {
    return withDefaults(await get<Partial<CashierSettings>>('/settings/cashier'))
  } catch {
    const snap = await readSnapshot().catch(() => null)
    return withDefaults(snap?.cashier)
  }
}
