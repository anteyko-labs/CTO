// Печать масляной книжки на ленте 80 мм (SPEC-16, SPEC-27) и QR-код на Телеграм-бот.
// После чека — «чек книжки»: только этот чек и только жидкости — что залито и следующая замена по
// пробегу; без фильтров, мастера, «Рекомендовано» и истории (ADR-115, ADR-116). Из карточки
// клиента — прежняя выписка: последняя замена по строкам, следующая по каждой строке и история.
import { printInFrame } from './printFrame'
import QRCode from 'qrcode'
import { get } from './api'
import { formatDate } from './format'
import { fluidText, isFluid, recordLines, recordMileage, recordText, slotLabel, slotNextText, slotsOfSale, slotsWithNext, type OilRecord, type SlotNext, type VehicleBook } from './oilBook'

const esc = (s: string) =>
  s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c] ?? c)

/** Ссылка на бота, если он подключён на сервере. */
export async function botLink(): Promise<string | null> {
  const r = await get<{ username: string | null }>('/telegram/bot').catch(() => ({ username: null }))
  return r.username ? `https://t.me/${r.username}` : null
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
  </style>`

const row = (label: string, value: string) => `<div class="row"><span>${esc(label)}</span><b>${esc(value)}</b></div>`

async function qrHtml(link: string | null): Promise<string> {
  if (!link) return ''
  const qr = await QRCode.toString(link, { type: 'svg', margin: 0, errorCorrectionLevel: 'M' })
  return `<div class="qr">${qr}</div><div class="muted">Сканируйте камерой телефона: история замен,<br>напоминание о замене и бонусные баллы в Телеграм</div>`
}

const plateHtml = (b: VehicleBook) =>
  `<div class="plate">${esc(b.plate)}${b.brand || b.model ? ` · ${esc([b.brand, b.model].filter(Boolean).join(' '))}` : ''}</div>`

/** Чек книжки после чека (ADR-116): машина, дата, пробег, «Залито» и «Следующая замена» по
 *  жидкостям этого чека — только пробег. */
async function saleCheckHtml(b: VehicleBook, r: OilRecord, saleId: string, link: string | null): Promise<string> {
  const filled = recordLines(r)
    .filter((l) => isFluid(l.slot))
    .map((l) => row(slotLabel(l.slot), fluidText(l)))
    .join('')
  const next = slotsOfSale(b, saleId)
    .filter((n) => isFluid(n.slot))
    .map((n) => row(n.label, slotNextText(n)))
    .join('')
  return `<!doctype html><html><head><meta charset="utf-8"><title>Масляная книжка ${esc(b.plate)}</title>${STYLE}</head><body>
    <h1>Avtodom</h1>
    <h2>Масляная книжка</h2>
    ${plateHtml(b)}
    ${row('Дата', formatDate(r.change_date))}
    ${recordMileage(r) !== null ? row('Пробег', recordMileage(r) ?? '') : ''}
    ${filled ? `<h3>Залито:</h3>${filled}` : ''}
    ${next ? `<div class="next">Следующая замена:${next}</div>` : ''}
    ${await qrHtml(link)}
  </body></html>`
}

/** `saleId` — книжка печатается после этого чека: «чек книжки» (ADR-116); без него — выписка. */
export async function oilBookHtml(
  b: VehicleBook,
  opts: { client?: string | null; master?: string | null; link: string | null; saleId?: string | null },
): Promise<string> {
  const own = opts.saleId ? b.records.find((r) => r.sale_id === opts.saleId) : undefined
  if (opts.saleId && own) return saleCheckHtml(b, own, opts.saleId, opts.link)
  const last = b.records[0]
  // Следующая замена по каждой строке книжки (SPEC-27), масло первым; только пробег (ADR-115).
  const next = slotsWithNext(b)
    .map((n: SlotNext) => row(n.label, slotNextText(n)))
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
      .map((l) => row(slotLabel(l.slot), l.text))
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
export async function printOilBook(b: VehicleBook, opts: { client?: string | null; master?: string | null; saleId?: string | null } = {}): Promise<void> {
  printInFrame(await oilBookHtml(b, { ...opts, link: await botLink() }))
}
