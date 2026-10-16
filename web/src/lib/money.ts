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

/**
 * Цена литра на розлив у масла в канистре: цена канистры × 1000 / объём в мл, до тыйына (ADR-074).
 * У масла одна цена — цена канистры. То же правило на сервере: `canister_pour_price`
 * в server/src/domain/money.rs. `null` при объёме не больше нуля.
 */
export const canisterPourPrice = (canisterPrice: number, containerMl: number): number | null =>
  containerMl > 0 ? divRound(canisterPrice * 1000, containerMl) : null

/** Цена литра на розлив товара: у масла — от цены канистры, у остального розлива нет. */
export const pourPricePerL = (p: { unit: string; container_ml: number | null; sale_price_tyiyn: number }): number | null =>
  p.unit === 'ml' && p.container_ml ? canisterPourPrice(p.sale_price_tyiyn, p.container_ml) : null
