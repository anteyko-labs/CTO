// Подбор кросс-номеров фильтра сервисом разбора (задача 002 П10, ADR-{B3.2}): в карточке товара и
// для всей категории. Режим и дневной лимит — «Настройки → Кросс-номера» (только владелец).
import { newOpId, post } from './api'

/** Записывать сразу (по умолчанию) или предлагать — сохраняет человек. */
export type CrossMode = 'auto' | 'suggest'

export const CROSS_MODES: { value: CrossMode; label: string; hint: string }[] = [
  { value: 'auto', label: 'Записывать сразу', hint: 'номера сразу в карточке с пометкой «подобрано автоматически»; лишний удаляется в карточке' },
  { value: 'suggest', label: 'Предлагать', hint: 'номера показываются с галочками, сохраняет человек' },
]

export interface CrossPickSettings {
  mode: CrossMode
  /** Обращений к сервису за день на филиал; 0 — подбор выключен. */
  daily_limit: number
  used_today: number
  /** Сервис разбора настроен на сервере. */
  available: boolean
}

export interface CrossPickResult {
  product_id: string
  mode: CrossMode
  /** Новые номера из ответа (которых у товара ещё нет). */
  found: string[]
  /** Записанные в карточку (режим «записывать сразу»). */
  added: string[]
  codes: string[]
  auto: string[]
  used_today: number
  daily_limit: number
  /** Сервис не ответил — номера не тронуты. */
  error: string | null
}

/** Подобрать кросс-номера товара (один запрос к сервису, `op_id` — без повтора). */
export const pickCross = (productId: string): Promise<CrossPickResult> => post<CrossPickResult>(`/products/${productId}/cross/pick`, { op_id: newOpId() })

/** Номер для сравнения — только буквы и цифры, верхний регистр (как `analogs::norm` на сервере). */
export function normCode(code: string): string {
  return code.replace(/[^0-9A-Za-zА-Яа-яЁё]/g, '').toUpperCase()
}

/** Номера из поля карточки: через запятую, точку с запятой или с новой строки. */
export function splitCodes(text: string): string[] {
  return text
    .split(/[,;\n]/)
    .map((c) => c.trim())
    .filter(Boolean)
}

/** Поле карточки с добавленными номерами: уже записанные (в любом написании) не повторяются. */
export function addCodes(text: string, codes: readonly string[]): string {
  const list = splitCodes(text)
  const seen = new Set(list.map(normCode))
  for (const c of codes) {
    const n = normCode(c)
    if (!n || seen.has(n)) continue
    seen.add(n)
    list.push(c.trim())
  }
  return list.join(', ')
}

/** «Подобрано 3 из 12, осталось сегодня 47» — для подбора по категории. */
export function pickProgress(done: number, all: number, r: Pick<CrossPickResult, 'used_today' | 'daily_limit'> | null): string {
  const left = r ? Math.max(0, r.daily_limit - r.used_today) : null
  return `Обработано ${done} из ${all}${left !== null ? `, подборов на сегодня осталось ${left}` : ''}`
}
