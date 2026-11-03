// «Настройки → Масляная книжка и печать» (задача 003 Н5, ADR-144): что касса печатает сама
// после чека «В сервис», что идёт в чек книжки и в блок книжки товарного чека, как пишется масло
// канистрой и разливом и идут ли фильтры на срок масла. У каждого филиала своя
// (`GET /settings/oil-book`); без сети — из снимка кассы. По умолчанию — как было.
import { get } from './api'
import { readSnapshot } from './offline'

/** Канистра и разлив одного товара — одной строкой: итог или итог с разбивкой. */
export type QtyStyle = 'total' | 'split'

export interface OilBookSettings {
  /** После чека «В сервис» печатать чек книжки (как было). */
  auto_book: boolean
  /** После чека «В сервис» печатать и товарный чек. */
  auto_receipt: boolean
  /** Блок «Масляная книжка» в товарном чеке (ADR-116). */
  receipt_block: boolean
  /** В чеке книжки и блоке — ещё фильтры; по умолчанию — да (задача 003 Н5). */
  with_filters: boolean
  /** Запись чека — только фильтры: печатать чек книжки. */
  print_filters_only: boolean
  qty_style: QtyStyle
  /** Масляный, воздушный, салонный фильтр с заменой масла — на пробег масла (ADR-117). */
  filters_follow_oil: boolean
  /** Чек «В сервис» без сети: после отправки на сервер печатать чек книжки (задача 003, хвост Н5). */
  offline_book: boolean
  /** После чека книжки — QR-наклейка на бота своей печатью (задача 024, ADR-187). */
  qr_sticker: boolean
}

export const OIL_BOOK_DEFAULTS: OilBookSettings = {
  auto_book: true,
  auto_receipt: false,
  receipt_block: true,
  with_filters: true,
  print_filters_only: false,
  qty_style: 'total',
  filters_follow_oil: true,
  offline_book: true,
  qr_sticker: true,
}

/** Настройка поверх значений по умолчанию: у старого снимка части полей нет. */
export function oilBookSettingsOf(raw: Partial<OilBookSettings> | null | undefined): OilBookSettings {
  const s = { ...OIL_BOOK_DEFAULTS, ...(raw ?? {}) }
  if (s.qty_style !== 'total' && s.qty_style !== 'split') s.qty_style = 'total'
  return s
}

let last: OilBookSettings = OIL_BOOK_DEFAULTS

/** Последняя прочитанная настройка (до первого чтения — по умолчанию): печать товарного чека
 *  берёт из неё блок книжки. */
export const cachedOilBookSettings = (): OilBookSettings => last

/** Настройка филиала: с сервера, без связи — из снимка, нет и его — по умолчанию. */
export async function loadOilBookSettings(): Promise<OilBookSettings> {
  try {
    last = oilBookSettingsOf(await get<OilBookSettings>('/settings/oil-book'))
  } catch {
    last = oilBookSettingsOf((await readSnapshot())?.oil_book)
  }
  return last
}

/** Что делать с чеком книжки, когда сервер принял чек, пробитый без сети: печатать сам, показать
 *  кнопку «Печать книжки» в уведомлении (печать выключена или окна кассы не видно) или ничего
 *  (не «В сервис», без машины, чек книжки сам не печатается). */
export function offlineBookPlan(body: Record<string, unknown>, s: Pick<OilBookSettings, 'auto_book' | 'offline_book'>, visible: boolean): 'print' | 'notice' | 'none' {
  if (body.sale_type !== 'service' || typeof body.vehicle_id !== 'string' || !body.vehicle_id) return 'none'
  if (!s.auto_book) return 'none'
  return s.offline_book && visible ? 'print' : 'notice'
}

/**
 * Что делает одна вкладка кассы с отправленным чеком (`plan` — по её видимости). Отправленный чек
 * узнают все вкладки устройства; сама печатает только одна видимая — первая взявшая чек (`claim` —
 * общая для вкладок запись), остальные видимые молчат. Скрытые и вкладки с выключенной печатью
 * показывают уведомление, если книжку ещё не напечатали (`finished` — «готово» от другой вкладки).
 * Печать не удалась — уведомление в печатавшей.
 */
export async function offlineBookTab(
  plan: 'print' | 'notice' | 'none',
  t: { claim: () => Promise<boolean>; print: () => Promise<void>; finished: () => boolean; done: () => void },
): Promise<'printed' | 'notice' | 'skip'> {
  if (plan === 'none') return 'skip'
  if (plan === 'print') {
    if (!(await t.claim())) return 'skip'
    try {
      await t.print()
      t.done()
      return 'printed'
    } catch {
      return t.finished() ? 'skip' : 'notice'
    }
  }
  return t.finished() ? 'skip' : 'notice'
}
