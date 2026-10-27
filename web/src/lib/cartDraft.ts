// Черновик чека кассы лежит в sessionStorage (см. Cashier.tsx): по нему обновление программы и
// большой снимок каталога ждут, пока чек пуст.

/** Пустой ли чек на кассе. */
export function cartIsEmpty(): boolean {
  try {
    const raw = sessionStorage.getItem('avtodom.cart')
    if (!raw) return true
    const d = JSON.parse(raw) as { lines?: unknown[] }
    return !d.lines || d.lines.length === 0
  } catch {
    return true
  }
}
