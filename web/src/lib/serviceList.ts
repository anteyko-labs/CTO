// Список услуг в окнах кассы «Вид замены» и «Услуги» (SPEC-36 §16).
import type { Service } from './types'

/** Плиток сразу — окно видно без ожидания; остальные дорисовываются при прокрутке (SPEC-36 §16:
 *  на моноблоке отрисовка 500 плиток занимает заметную долю секунды). */
export const FIRST_TILES = 30

/** Свежий список тот же, что показан: окно не перерисовывается заново. */
export function sameServices(a: Service[] | null, b: Service[]): boolean {
  return a !== null && a.length === b.length && JSON.stringify(a) === JSON.stringify(b)
}
