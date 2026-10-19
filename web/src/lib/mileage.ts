// Пробег в км или милях (SPEC-29 §22, ADR-084): единица — у машины, интервалы книжки — в км.
// Перевод одной функцией, только целые: 1 миля = 1,609344 км (миллионные доли), округление
// половиной от нуля — `divRound`, как во всём учёте. То же правило на сервере:
// `server/src/domain/mileage.rs`.
import { divRound } from './money'

export type MileageUnit = 'km' | 'mi'

const MI_MICRO_KM = 1_609_344
const MICRO = 1_000_000

export const miToKm = (mi: number): number => divRound(mi * MI_MICRO_KM, MICRO)
export const kmToMi = (km: number): number => divRound(km * MICRO, MI_MICRO_KM)

/** Пробег в единице машины → км. */
export const toKm = (value: number, unit: MileageUnit): number => (unit === 'mi' ? miToKm(value) : value)

/** Км → пробег в единице машины. */
export const fromKm = (km: number, unit: MileageUnit): number => (unit === 'mi' ? kmToMi(km) : km)

/** Единица словом: «км», «миль». */
export const unitLabel = (unit: MileageUnit | undefined): string => (unit === 'mi' ? 'миль' : 'км')

/** «93 000 км», «57 790 миль». */
export function mileageText(value: number, unit: MileageUnit | undefined = 'km'): string {
  return `${value.toLocaleString('ru-RU').replace(/\s/g, ' ')} ${unitLabel(unit)}`
}
