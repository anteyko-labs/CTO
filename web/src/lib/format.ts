// Перевод целых тыйынов и миллилитров в текст и обратно. Только для показа и ввода:
// расчёты выполняет сервер, дробные числа здесь не используются.

const group = (digits: string): string => digits.replace(/\B(?=(\d{3})+(?!\d))/g, '\u00a0')

/** 123450 → «1 234,50 с» */
export function formatSom(tyiyn: number, withUnit = true): string {
  const sign = tyiyn < 0 ? '−' : ''
  const abs = Math.abs(tyiyn)
  const whole = Math.trunc(abs / 100)
  const frac = abs % 100
  const text = `${sign}${group(String(whole))},${String(frac).padStart(2, '0')}`
  return withUnit ? `${text}\u00a0с` : text
}

/** 2500 → «2,5 л», 4000 → «4 л» */
export function formatLiters(ml: number): string {
  const sign = ml < 0 ? '−' : ''
  const abs = Math.abs(ml)
  const whole = Math.trunc(abs / 1000)
  const frac = String(abs % 1000).padStart(3, '0').replace(/0+$/, '')
  return `${sign}${group(String(whole))}${frac ? ',' + frac : ''}\u00a0л`
}

/** Дата без времени: «2026-10-09» → «09.10.2026». */
export function formatDate(iso: string): string {
  const [y, m, d] = iso.slice(0, 10).split('-')
  return `${d}.${m}.${y}`
}

/** Вес для показа: 12 350 г → «12,35 кг». */
export function formatKg(g: number): string {
  return formatLiters(g).replace(/л$/, 'кг')
}

/** Остаток масла: «14,5 л · 3 кан. по 4 л + 2,5 л» — считаем литрами, тара в расшифровке. */
export function formatOilStock(ml: number, containerMl: number): string {
  const total = formatLiters(ml)
  if (ml <= 0 || containerMl <= 0) return total
  const full = Math.trunc(ml / containerMl)
  const rest = ml % containerMl
  const parts: string[] = []
  if (full > 0) parts.push(`${full} кан. по ${formatLiters(containerMl)}`)
  if (rest > 0) parts.push(formatLiters(rest))
  return parts.length > 1 || full > 1 ? `${total} · ${parts.join(' + ')}` : total
}

/** Разбор десятичной строки в целое с `scale` знаками: «12,5» при scale=2 → 1250. */
function parseFixed(input: string, scale: number): number | null {
  const s = input.trim().replace(/\s/g, '').replace(',', '.')
  const m = /^(\d+)(?:\.(\d*))?$/.exec(s)
  if (!m) return null
  const frac = m[2] ?? ''
  if (frac.length > scale) return null
  const value = Number(m[1]) * 10 ** scale + Number(frac.padEnd(scale, '0') || '0')
  return Number.isSafeInteger(value) ? value : null
}

/** «1 234,5» → 123450 тыйын; null при ошибке. */
export const parseSom = (input: string): number | null => parseFixed(input, 2)

/** «2,5» → 2500 мл; null при ошибке. */
export const parseLiters = (input: string): number | null => parseFixed(input, 3)

/** Сомы для поля ввода: 123450 → «1234,50». */
export function somInput(tyiyn: number): string {
  return formatSom(tyiyn, false).replace(/\u00a0/g, '')
}

export function formatDateTime(iso: string): string {
  return new Date(iso).toLocaleString('ru-RU', { timeZone: 'Asia/Bishkek', dateStyle: 'short', timeStyle: 'short' })
}

/** Сегодняшняя дата в Бишкеке в формате YYYY-MM-DD. */
export function todayBishkek(): string {
  return new Date().toLocaleDateString('sv-SE', { timeZone: 'Asia/Bishkek' })
}

/** Текущий час в Бишкеке (0–23) — для графика «сегодня по часам». */
export function hourBishkek(): number {
  return Number(new Date().toLocaleString('en-GB', { timeZone: 'Asia/Bishkek', hour: '2-digit', hour12: false })) % 24
}

/** Сдвиг даты YYYY-MM-DD на `days` дней; часовой пояс устройства не участвует. */
export function shiftDate(date: string, days: number): string {
  const d = new Date(`${date}T12:00:00Z`)
  d.setUTCDate(d.getUTCDate() + days)
  return d.toISOString().slice(0, 10)
}
