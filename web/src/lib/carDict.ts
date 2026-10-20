// Справочник марок и моделей машин (SPEC-29 §21, ADR-083): выбор при добавлении машины.
import type { CarMake } from './types'

export type { MileageUnit } from './mileage'

/** Модели марки по введённому названию (без учёта регистра и пробелов по краям); марки нет — пусто. */
export function modelsOf(makes: readonly CarMake[], brand: string): CarMake['models'] {
  const key = brand.trim().toLowerCase()
  if (!key) return []
  return makes.find((m) => m.name.toLowerCase() === key)?.models ?? []
}

const carKey = (s: string) => s.toLowerCase().replace(/[\s-]+/g, '')

/**
 * Марка или модель в том написании, как в справочнике: «rx 350» → «RX 350», «toyota» → «Toyota»
 * (сравнение без учёта регистра, пробелов и дефисов). Нет в справочнике и набрано строчными —
 * слова с заглавной, короткие (до 3 букв) и с цифрами — целиком заглавными: «land cruiser» →
 * «Land Cruiser», «rav4» → «RAV4», «bmw» → «BMW». Набранное с заглавными оставляем как есть.
 */
export function carName(text: string, known: readonly string[]): string {
  const t = text.trim().replace(/\s+/g, ' ')
  if (!t) return ''
  const key = carKey(t)
  const hit = known.find((k) => carKey(k) === key)
  if (hit) return hit
  if (t !== t.toLowerCase()) return t
  return t
    .split(' ')
    .map((w) =>
      w
        .split('-')
        .map((p) => (/\d/.test(p) || p.length <= 3 ? p.toUpperCase() : p.charAt(0).toUpperCase() + p.slice(1)))
        .join('-'),
    )
    .join(' ')
}
