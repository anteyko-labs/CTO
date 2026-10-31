// Печать масляной книжки на ленте 80 мм (SPEC-16, SPEC-27) и QR-код на Телеграм-бот.
// После чека — «чек книжки»: только этот чек — что поменяли по видам замены (услугам) и следующая
// замена по пробегу; без мастера, «Рекомендовано» и истории (ADR-115, ADR-116). Подпись — товар, а
// не строка книжки; фильтры — по настройке, по умолчанию с ними (задача 003 Н5). Из карточки
// клиента — прежняя выписка: последняя замена по строкам, следующая по каждой строке и история.
import { printInFrame } from './printFrame'
import QRCode from 'qrcode'
import { get } from './api'
import { formatDate } from './format'
import { OIL_BOOK_DEFAULTS, type OilBookSettings } from './oilBookSettings'
import {
  bookGroups,
  bookGroupsHtml,
  isFluid,
  lineGroup,
  lineTitle,
  recordLines,
  recordMileage,
  recordText,
  slotKey,
  slotNextText,
  slotsOfSale,
  slotsWithNext,
  slotTitle,
  type BookLine,
  type OilRecord,
  type SlotNext,
  type VehicleBook,
} from './oilBook'

const esc = (s: string) =>
  s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c] ?? c)

let botCache: Promise<string | null> | null = null

/** Ссылка на бота, если он подключён на сервере. Спрашивается один раз за сессию (SPEC-36 §16):
 *  книжка печатается после каждого чека «В сервис»; ключ бота меняется только
 *  перезапуском сервера. Без ответа сервера не запоминается — следующая
 *  печать спросит снова. */
export function botLink(): Promise<string | null> {
  botCache ??= get<{ username: string | null }>('/telegram/bot').then(
    (r) => (r.username ? `https://t.me/${r.username}` : null),
    () => {
      botCache = null
      return null
    },
  )
  return botCache
}

const STYLE = `<style>
    @page { size: 80mm auto; margin: 3mm; }
    body { font-family: Arial, sans-serif; font-size: 10pt; margin: 0; width: 72mm; }
    h1 { font-size: 13pt; text-align: center; margin: 0; }
    h2 { font-size: 11pt; text-align: center; margin: 0 0 2mm; font-weight: normal; }
    h3 { font-size: 10pt; margin: 2mm 0 0.5mm; }
    .plate { font-size: 14pt; font-weight: bold; text-align: center; border: 1.5px solid #000; padding: 1mm; margin: 2mm 0; }
    .row { display: flex; justify-content: space-between; gap: 2mm; padding: 0.5mm 0; }
    .row b { text-align: right; }
    .next { border: 1.5px dashed #000; padding: 2mm; margin: 2mm 0; text-align: center; }
    .next .row { text-align: left; }
    .next .row b { font-size: 10.5pt; }
    table { width: 100%; border-collapse: collapse; font-size: 8.5pt; }
    td { padding: 0.5mm 1mm 0.5mm 0; vertical-align: top; }
    .qr { text-align: center; margin-top: 3mm; }
    .qr svg { width: 32mm; height: 32mm; }
    .muted { color: #333; font-size: 8.5pt; text-align: center; }
    .bk { border-top: 1px dashed #000; padding: 1mm 0; }
    .bk .grp { font-weight: bold; }
    .bk .nx { font-weight: bold; margin-top: 0.5mm; }
  </style>`

const row = (label: string, value: string) => `<div class="row"><span>${esc(label)}</span><b>${esc(value)}</b></div>`

async function qrHtml(link: string | null): Promise<string> {
  if (!link) return ''
  const qr = await QRCode.toString(link, { type: 'svg', margin: 0, errorCorrectionLevel: 'M' })
  return `<div class="qr">${qr}</div><div class="muted">Сканируйте камерой телефона: история замен,<br>напоминание о замене и бонусные баллы в Телеграм</div>`
}

const plateHtml = (b: VehicleBook) =>
  `<div class="plate">${esc(b.plate)}${b.brand || b.model ? ` · ${esc([b.brand, b.model].filter(Boolean).join(' '))}` : ''}</div>`

/** Что печатает чек книжки (задача 003 Н5): `with_filters` — и фильтры (по умолчанию — да);
 *  `print_filters_only` — запись только с фильтрами печатается и при выключенных фильтрах. Нет
 *  поля — как по умолчанию (`OIL_BOOK_DEFAULTS`). */
export type BookPrint = Partial<Pick<OilBookSettings, 'with_filters' | 'print_filters_only'>>

const isFilter = (slot: string) => slot.endsWith('_filter')

/** Строки, которые идут в чек книжки этой записи: жидкости, фильтры — по настройке, работы
 *  «прочее» по услуге — своим названием и сроком (задача 003 Н5). Как `oil_book::sale_book`. */
function printedLine(r: OilRecord, s: BookPrint): (l: BookLine) => boolean {
  const fluids = recordLines(r).some((l) => isFluid(l.slot))
  const filters = (s.with_filters ?? OIL_BOOK_DEFAULTS.with_filters) || (!fluids && Boolean(s.print_filters_only))
  return (l) => isFluid(l.slot) || (filters && isFilter(l.slot)) || (l.slot === 'other' && Boolean(l.service_id ?? l.service))
}

/** Чек книжки после чека (ADR-116, задача 003 Н5): машина, дата, пробег и что поменяли по видам
 *  замены — товаром («Mobil 1 5W-30 · 4 л», «W712 (масляный) · 1 шт») — со следующей заменой по
 *  пробегу. */
async function saleCheckHtml(b: VehicleBook, r: OilRecord, saleId: string, link: string | null, s: BookPrint): Promise<string> {
  const printed = printedLine(r, s)
  const all = recordLines(r)
  const next = new Map(slotsOfSale(b, saleId).map((n) => [slotKey(n), slotNextText(n)]))
  const items = all.filter(printed).map((l) => ({ title: lineTitle(l), group: lineGroup(l, all), next: next.get(slotKey(l)) ?? '' }))
  return `<!doctype html><html><head><meta charset="utf-8"><title>Масляная книжка ${esc(b.plate)}</title>${STYLE}</head><body>
    <h1>Avtodom</h1>
    <h2>Масляная книжка</h2>
    ${plateHtml(b)}
    ${row('Дата', formatDate(r.change_date))}
    ${recordMileage(r) !== null ? row('Пробег', recordMileage(r) ?? '') : ''}
    ${bookGroupsHtml(bookGroups(items))}
    ${await qrHtml(link)}
  </body></html>`
}

/** Запись этого чека в книжке — её панель «Записано в масляную книжку» на кассе видна всегда
 *  (задача 003 Н5), даже если печатать нечего. */
export function saleRecordOf(b: VehicleBook | null | undefined, saleId: string): OilRecord | null {
  return b?.records.find((r) => r.sale_id === saleId) ?? null
}

/** Запись этого чека, если в ней есть что печатать (`printedLine`: масла и жидкости, фильтры по
 *  настройке, работы «прочее»), — её печатает «чек книжки» (задача 003 Н5). Записи нет (ADR-130) или
 *  в ней только фильтры при выключенных фильтрах — null: на ленте остались бы госномер, дата и QR. */
export function saleCheckOf(b: VehicleBook, saleId: string, s: BookPrint = {}): OilRecord | null {
  const own = saleRecordOf(b, saleId)
  if (!own) return null
  return recordLines(own).some(printedLine(own, s)) ? own : null
}

/** `saleId` — книжка печатается после этого чека: «чек книжки» (ADR-116); печатать нечего
 *  (`saleCheckOf`) — null, после чека прошлые замены не печатаются (ADR-130). Без `saleId` — выписка. */
export async function oilBookHtml(
  b: VehicleBook,
  opts: { client?: string | null; master?: string | null; link: string | null; saleId?: string | null; print?: BookPrint },
): Promise<string | null> {
  if (opts.saleId) {
    const own = saleCheckOf(b, opts.saleId, opts.print)
    return own ? saleCheckHtml(b, own, opts.saleId, opts.link, opts.print ?? {}) : null
  }
  const last = b.records[0]
  // Следующая замена по каждой строке книжки (SPEC-27), масло первым; только пробег (ADR-115);
  // подпись — что залили в последний раз (задача 003 Н5).
  const next = slotsWithNext(b)
    .map((n: SlotNext) => row(slotTitle(n), slotNextText(n)))
    .join('')
  const history = b.records
    .slice(1, 4)
    .map(
      (r) =>
        `<tr><td>${esc(formatDate(r.change_date))}</td><td>${esc(recordMileage(r) ?? '')}</td><td>${esc(recordText(r))}</td></tr>`,
    )
    .join('')
  return `<!doctype html><html><head><meta charset="utf-8"><title>Масляная книжка ${esc(b.plate)}</title>${STYLE}</head><body>
    <h1>Avtodom</h1>
    <h2>Масляная книжка</h2>
    ${plateHtml(b)}
    ${opts.client ? row('Владелец', opts.client) : ''}
    ${
      last
        ? `${row('Замена', formatDate(last.change_date))}
    ${recordMileage(last) !== null ? row('Пробег', recordMileage(last) ?? '') : ''}
    ${recordLines(last)
      .map((l) => `<div class="row"><span>${esc(lineTitle(l))}</span></div>`)
      .join('')}
    ${opts.master ? row('Мастер', opts.master) : ''}
    ${last.declined?.trim() ? row('Рекомендовано', last.declined.trim()) : ''}`
        : '<div class="muted">Замен пока не было</div>'
    }
    ${next ? `<div class="next">Следующая замена${next}</div>` : ''}
    ${history ? `<div class="muted">Прошлые замены</div><table>${history}</table>` : ''}
    ${await qrHtml(opts.link)}
  </body></html>`
}

/** Печать на чековом принтере через диалог браузера; `saleId` — после этого чека (ADR-116). */
export async function printOilBook(
  b: VehicleBook,
  opts: { client?: string | null; master?: string | null; saleId?: string | null; print?: BookPrint } = {},
): Promise<void> {
  const html = await oilBookHtml(b, { ...opts, link: await botLink() })
  if (html) printInFrame(html)
}
