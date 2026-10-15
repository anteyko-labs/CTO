// Тексты сводки владельца (SPEC-08): строки заработка и сравнения. Только показ, расчёты — на сервере.
import { formatSom } from './format'
import type { StaffPay } from './types'

/** Вид начисления словами; количество — только там, где оно что-то говорит. */
const KIND: Record<string, (count: number) => string> = {
  service_fee: (n) => `замены ×${n}`,
  revenue_percent: () => '% с чеков',
  shift_fee: () => 'за смену',
  monthly_salary: () => 'оклад',
  bonus: () => 'премия',
  penalty: () => 'удержание',
  shortage: () => 'удержание недостачи',
}

/** «% с чеков 8 с · замены ×2 60 с» — за что начислено, коротко. */
export function payReason(items: StaffPay['items']): string {
  return items.map((i) => `${(KIND[i.kind] ?? (() => i.kind))(i.count)} ${formatSom(i.amount_tyiyn)}`).join(' · ')
}

/** Только те, кому что-то начислено (плюс или минус), больше — выше. */
export function earnedList(staff: StaffPay[]): StaffPay[] {
  return staff.filter((p) => p.items.length > 0).sort((a, b) => b.total_tyiyn - a.total_tyiyn || a.name.localeCompare(b.name, 'ru'))
}

export interface CompareLine {
  text: string
  /** Направление: рост, падение или без изменений — для цвета стрелки. */
  tone: 'up' | 'down' | 'flat'
  /** Полная подпись для подсказки при наведении. */
  title: string
}

/**
 * Короткое сравнение под плиткой: «▲ 40 % к вчера». Не с чем сравнить (вчера к этому часу
 * было 0) — null. Разница больше 200 % в процентах ничего не говорит: показываем, сколько было.
 */
export function vsYesterday(now: number, base: number, fmt: (v: number) => string): CompareLine | null {
  if (base === 0) return null
  const pct = Math.round(((now - base) * 100) / base)
  const tone = pct > 0 ? 'up' : pct < 0 ? 'down' : 'flat'
  const arrow = pct > 0 ? '▲' : pct < 0 ? '▼' : '='
  const title = `вчера к этому часу: ${fmt(base)}`
  if (Math.abs(pct) > 200) return { text: `${arrow} вчера: ${fmt(base)}`, tone, title }
  return { text: `${arrow} ${Math.abs(pct)} % к вчера`, tone, title }
}

/** «нед. назад: 9 200 с» — тот же день неделю назад к этому часу; нуль не показываем. */
export function weekAgo(base: number, fmt: (v: number) => string): string | null {
  return base === 0 ? null : `нед. назад: ${fmt(base)}`
}

/** Строка «лесенки» прибыли: вычет — «−85 400 с», знак прямо перед числом. */
export function minusSom(value: number): string {
  return formatSom(value === 0 ? 0 : -value)
}

/** Сумма со своим знаком: «+300 с» добавляет к прибыли, «−300 с» уменьшает. */
export function signedSom(value: number): string {
  return value > 0 ? `+${formatSom(value)}` : formatSom(value)
}
