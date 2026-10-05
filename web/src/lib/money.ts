// Предварительный расчёт суммы для показа в кассе до проведения.
// Повторяет правило сервера (server/src/domain/money.rs): округление половины от нуля.
// Итог чека определяет сервер.

/** n / d с округлением половины от нуля; d > 0, |n| ≤ 2^53. */
export function divRound(n: number, d: number): number {
  const q = Math.trunc(n / d)
  const r = n - q * d
  return 2 * Math.abs(r) >= d ? q + Math.sign(n) : q
}

/** Сумма розлива: мл × цена за литр / 1000. */
export const pourAmount = (ml: number, pricePerLiter: number): number => divRound(ml * pricePerLiter, 1000)
