// Справочник марок и моделей машин (SPEC-29 §21, ADR-083): выбор при добавлении машины.
import type { CarMake } from './types'

export type { MileageUnit } from './mileage'

/** Модели марки по введённому названию (без учёта регистра и пробелов по краям); марки нет — пусто. */
export function modelsOf(makes: readonly CarMake[], brand: string): CarMake['models'] {
  const key = brand.trim().toLowerCase()
  if (!key) return []
  return makes.find((m) => m.name.toLowerCase() === key)?.models ?? []
}
