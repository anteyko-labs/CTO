// Печать масляной книжки (SPEC-16, SPEC-27) и QR-наклейки на Телеграм-бот.
// После чека — «чек книжки»: только этот чек — что поменяли по видам замены (услугам) и следующая
// замена по пробегу; без мастера, «Рекомендовано» и истории (ADR-115, ADR-116). Подпись — товар, а
// не строка книжки; фильтры — по настройке, по умолчанию с ними (задача 003 Н5). Из карточки
// клиента — выписка на ленте 80 мм: последняя замена по строкам и следующая по каждой строке.
// QR в книжке нет: он — своей наклейкой и своей печатью (задача 024, ADR-187).
import { printInFrame } from './printFrame'
import QRCode from 'qrcode'
import { get, newOpId, post } from './api'
import { toast } from '../components/ui'
import { formatDate } from './format'
import { loadOilBookSettings, OIL_BOOK_DEFAULTS, type OilBookSettings } from './oilBookSettings'
import {
  bookGroups,
  isFluid,
  lineGroup,
  lineTitle,
  recordLines,
  recordMileage,
  slotKey,
  slotNextText,
  slotsOfSale,
  slotsWithNext,
  slotTitle,
  type BookItem,
  type BookLine,
  type OilRecord,
  type SlotNext,
  type VehicleBook,
} from './oilBook'

const esc = (s: string) =>
  s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c] ?? c)

let botCache: Promise<string | null> | null = null

/** Ссылка на бота, если он подключён на сервере. Спрашивается один раз за сессию (SPEC-36 §16):
 *  QR-наклейка печатается после каждого чека «В сервис»; ключ бота меняется только
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
    .muted { color: #333; font-size: 8.5pt; text-align: center; }
    .bk { border-top: 1px dashed #000; padding: 1mm 0; }
    .bk .grp { font-weight: bold; }
    .bk .nx { font-weight: bold; margin-top: 0.5mm; }
  </style>`

const row = (label: string, value: string) => `<div class="row"><span>${esc(label)}</span><b>${esc(value)}</b></div>`

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

/** Наклейка Xprinter XP-Q80A 58×40 мм (задача 009): драйвер POS80 в режиме «Label Paper» — одна
 *  страница на наклейку. Разметка подобрана пробной печатью на кассе 02.11.2026. */
const LABEL_STYLE = `<style>
    @page { size: 58mm 40mm; margin: 0; }
    body { font-family: Arial, sans-serif; margin: 0; }
    .lbl { width: 58mm; height: 40mm; box-sizing: border-box; padding: 2.5mm 3mm; overflow: hidden; page-break-after: always; break-after: page; }
    .lbl:last-child { page-break-after: auto; break-after: auto; }
    .lbl.nx { padding-top: 1.5mm; }
    .hd { display: flex; justify-content: space-between; align-items: baseline; font-size: 7.5pt; }
    .hd b { font-size: 9pt; }
    .plate { font-size: 13pt; font-weight: bold; text-align: center; border: 1.4px solid #000; padding: 0.6mm; margin: 1.5mm 0 0.5mm; line-height: 1.1; }
    .car { font-size: 8pt; text-align: center; margin-bottom: 3.5mm; }
    .row { display: flex; justify-content: space-between; font-size: 9pt; line-height: 1.35; }
    .ttl { font-size: 9pt; font-weight: bold; margin-bottom: 1mm; }
    .ln { font-size: 7.5pt; line-height: 1.25; margin-bottom: 0.5mm; }
  </style>`

/** Наклейки книжки: что залито и что менять дальше по видам замены. Пустой вид — без строки. */
export function labelLines(items: readonly BookItem[]) {
  const groups = bookGroups(items)
  const filled = groups.flatMap((g) => {
    const titles = g.items.map((i) => i.title).filter((t) => t !== g.head)
    return [g.head ? `${g.head}${titles.length ? ': ' + titles.join(', ') : ''}` : titles.join(', ')].filter(Boolean)
  })
  const next = groups.flatMap((g) =>
    g.next ? [`${g.head || g.items[0]?.title || ''} — ${g.next}`] : g.items.filter((i) => i.next).map((i) => `${i.title} — ${i.next}`),
  )
  return { filled, next }
}

/** Чек книжки после чека (ADR-116, задача 003 Н5, задача 009): три наклейки 58×40 мм — на первой
 *  машина, дата и пробег, на второй что залито, на третьей следующая замена по видам (без QR —
 *  он своей наклейкой, задача 024). */
function saleCheckHtml(b: VehicleBook, r: OilRecord, saleId: string, s: BookPrint): string {
  const printed = printedLine(r, s)
  const all = recordLines(r)
  const next = new Map(slotsOfSale(b, saleId).map((n) => [slotKey(n), slotNextText(n)]))
  const items = all.filter(printed).map((l) => ({ title: lineTitle(l), group: lineGroup(l, all), next: next.get(slotKey(l)) ?? '' }))
  const { filled, next: nexts } = labelLines(items)
  const mileage = recordMileage(r)
  const car = [b.brand, b.model].filter(Boolean).join(' ')
  const lines = (list: readonly string[]) => list.map((t) => `<div class="ln">${esc(t)}</div>`).join('')
  return `<!doctype html><html><head><meta charset="utf-8"><title>Масляная книжка ${esc(b.plate)}</title>${LABEL_STYLE}</head><body>
    <div class="lbl">
      <div class="hd"><b>Avtodom</b><span>Масляная книжка</span></div>
      <div class="plate">${esc(b.plate)}</div>
      <div class="car">${esc(car)}</div>
      ${row('Дата', formatDate(r.change_date))}
      ${mileage !== null ? row('Пробег', mileage ?? '') : ''}
    </div>
    <div class="lbl">
      <div class="ttl">Залито и заменено · ${esc(b.plate)}</div>
      ${lines(filled)}
    </div>
    <div class="lbl nx">
      <div class="ttl">Следующая замена</div>
      ${nexts.length ? lines(nexts) : '<div class="ln">—</div>'}
    </div>
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
 *  (`saleCheckOf`) — null, после чека прошлые замены не печатаются (ADR-130). Без `saleId` — выписка
 *  без «Прошлых замен» и QR (задача 024). */
export function oilBookHtml(
  b: VehicleBook,
  opts: { client?: string | null; master?: string | null; saleId?: string | null; print?: BookPrint } = {},
): string | null {
  if (opts.saleId) {
    const own = saleCheckOf(b, opts.saleId, opts.print)
    return own ? saleCheckHtml(b, own, opts.saleId, opts.print ?? {}) : null
  }
  const last = b.records[0]
  // Следующая замена по каждой строке книжки (SPEC-27), масло первым; только пробег (ADR-115);
  // подпись — что залили в последний раз (задача 003 Н5).
  const next = slotsWithNext(b)
    .map((n: SlotNext) => row(slotTitle(n), slotNextText(n)))
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
  </body></html>`
}

/** QR-наклейка 58×40 мм на Телеграм-бот (задача 024, ADR-187): QR 22 мм слева, подпись справа.
 *  Код попроще (`L`) и чёткие края — на термонаклейке телефон читает (ADR-180). */
export async function qrStickerHtml(link: string): Promise<string> {
  const qr = await QRCode.toString(link, { type: 'svg', margin: 0, errorCorrectionLevel: 'L' })
  return `<!doctype html><html><head><meta charset="utf-8"><title>QR Телеграм-бота</title><style>
    @page { size: 58mm 40mm; margin: 0; }
    body { font-family: Arial, sans-serif; margin: 0; }
    .lbl { width: 58mm; height: 40mm; box-sizing: border-box; padding: 2.5mm 3mm; display: flex; gap: 2.5mm; align-items: center; overflow: hidden; }
    .qr svg { width: 22mm; height: 22mm; display: block; shape-rendering: crispEdges; }
    .t { flex: 1; min-width: 0; font-size: 7.5pt; line-height: 1.2; }
    .t b { display: block; font-size: 10pt; margin-bottom: 1mm; }
    .t .m { font-weight: bold; font-size: 8pt; margin-bottom: 1.5mm; }
  </style></head><body><div class="lbl"><div class="qr">${qr}</div><div class="t"><b>Avtodom</b><div class="m">Масляная книжка в Телеграм</div><div>История замен, напоминание и бонусные баллы. Наведите камеру телефона</div></div></div></body></html>`
}

/** QR-наклейка своей печатью (задача 024); бот не подключён — печатать нечего: false. */
export async function printBotQr(): Promise<boolean> {
  const link = await botLink()
  if (!link) return false
  printInFrame(await qrStickerHtml(link))
  return true
}

/** Печать через диалог браузера; `saleId` — после этого чека (ADR-116). `qr` — после книжки ещё
 *  QR-наклейка отдельной печатью (задача 024): два задания, сначала книжка. */
type BookOpts = { client?: string | null; master?: string | null; saleId?: string | null; print?: BookPrint; qr?: boolean }

/** Книжка — на станцию книжки (задача 027): только если она в сети, иначе задание ждало бы в
 *  очереди и вышло бы вторым листом, когда станцию включат. Нет связи, ошибка — `false`. */
async function toBookStation(vehicleId: string, opts: BookOpts): Promise<boolean> {
  try {
    const list = await get<{ station_online: boolean }>('/label-jobs?size=book')
    if (!list.station_online) {
      toast('Станция книжки не в сети — книжка печатается здесь', 'error')
      return false
    }
    await post('/label-jobs/book', {
      op_id: newOpId(),
      vehicle_id: vehicleId,
      sale_id: opts.saleId ?? null,
      client: opts.client ?? null,
      master: opts.master ?? null,
      qr: Boolean(opts.qr),
    })
    toast('Книжка ушла на принтер книжки')
    return true
  } catch {
    toast('Станция книжки недоступна — книжка печатается здесь', 'error')
    return false
  }
}

export async function printOilBook(b: VehicleBook, opts: BookOpts = {}): Promise<void> {
  const html = oilBookHtml(b, opts)
  if (!html) return
  // Станция книжки — по настройке «Масляная книжка и печать» (задача 027); без связи — здесь.
  if ((await loadOilBookSettings()).book_station && navigator.onLine && (await toBookStation(b.vehicle_id, opts))) return
  printInFrame(html)
  if (opts.qr) await printBotQr()
}
