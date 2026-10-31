// Пересчёт строки ревизии несколькими людьми (задача 003, Н14): поле «+ добавить» или «= итог»
// и журнал «кто сколько посчитал».

/** `add` — прибавить найденное в своём месте; `set` — записать итог строки. */
export type CountMode = 'add' | 'set'

/** Запись журнала строки с сервера: прибавил, записал итог, убрал строку. */
export interface CountEntry {
  by: string
  mode: 'add' | 'set' | 'clear'
  qty: number
}

interface CountLine {
  unit: string
  container_ml: number | null
  counted_qty: number | null
}

/**
 * Режим поля, пока человек его не выбрал сам. Строку уже кто-то считал — «+ добавить»: человек
 * вводит то, что нашёл в своём месте; не считали — «= итог». Масло в таре — всегда прибавка:
 * канистры считают сканом, вскрытую канистру вписывают литрами.
 */
export function defaultCountMode(l: CountLine): CountMode {
  if (l.unit === 'ml' && l.container_ml) return 'add'
  return (l.counted_qty ?? 0) > 0 ? 'add' : 'set'
}

/** Своя запись — сразу в журнал на экране: прибавки одного человека подряд — одной записью. */
export function appendCount(log: CountEntry[], e: CountEntry): CountEntry[] {
  const last = log[log.length - 1]
  if (last && e.mode === 'add' && last.mode === 'add' && last.by === e.by) return [...log.slice(0, -1), { ...last, qty: last.qty + e.qty }]
  return [...log, e]
}

/** «Айбек +2 шт, Нурлан +1 шт, Азиз = 5 шт»; `fmt` — количество в единицах товара. */
export function countLogText(log: CountEntry[], fmt: (qty: number) => string): string {
  return log.map((e) => (e.mode === 'add' ? `${e.by} +${fmt(e.qty)}` : e.mode === 'set' ? `${e.by} = ${fmt(e.qty)}` : `${e.by} убрал`)).join(', ')
}
