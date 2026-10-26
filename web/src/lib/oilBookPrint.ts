// Печать масляной книжки на ленте 80 мм: последняя замена по строкам книжки, следующая по каждой
// строке и QR-код на Телеграм-бот (SPEC-16, SPEC-27). После чека — следующая по строкам этого чека,
// остальные строки — «По прошлым заменам» с датой замены (ADR-111).
import { printInFrame } from './printFrame'
import QRCode from 'qrcode'
import { get } from './api'
import { formatDate } from './format'
import { recordLines, recordMileage, recordText, slotLabel, slotNextText, slotsBeforeSale, slotsOfSale, slotsWithNext, type SlotNext, type VehicleBook } from './oilBook'

const esc = (s: string) =>
  s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c] ?? c)

/** Ссылка на бота, если он подключён на сервере. */
export async function botLink(): Promise<string | null> {
  const r = await get<{ username: string | null }>('/telegram/bot').catch(() => ({ username: null }))
  return r.username ? `https://t.me/${r.username}` : null
}

/** `saleId` — книжка печатается после этого чека: следующая замена — по его строкам. */
export async function oilBookHtml(
  b: VehicleBook,
  opts: { client?: string | null; master?: string | null; link: string | null; saleId?: string | null },
): Promise<string> {
  const last = b.records[0]
  // Следующая замена по каждой строке книжки (SPEC-27), масло первым; после чека — по строкам чека.
  const row = (n: SlotNext, label: string) => `<div class="row"><span>${esc(label)}</span><b>${esc(slotNextText(n))}</b></div>`
  const next = (opts.saleId ? slotsOfSale(b, opts.saleId) : slotsWithNext(b)).map((n) => row(n, n.label)).join('')
  const before = (opts.saleId ? slotsBeforeSale(b, opts.saleId) : [])
    .map((n) => row(n, n.last_date ? `${n.label} (замена ${formatDate(n.last_date)})` : n.label))
    .join('')
  const qr = opts.link ? await QRCode.toString(opts.link, { type: 'svg', margin: 0, errorCorrectionLevel: 'M' }) : ''
  const history = b.records
    .slice(1, 4)
    .map(
      (r) =>
        `<tr><td>${esc(formatDate(r.change_date))}</td><td>${esc(recordMileage(r) ?? '')}</td><td>${esc(recordText(r))}</td></tr>`,
    )
    .join('')
  return `<!doctype html><html><head><meta charset="utf-8"><title>Масляная книжка ${esc(b.plate)}</title><style>
    @page { size: 80mm auto; margin: 3mm; }
    body { font-family: Arial, sans-serif; font-size: 10pt; margin: 0; width: 72mm; }
    h1 { font-size: 13pt; text-align: center; margin: 0; }
    h2 { font-size: 11pt; text-align: center; margin: 0 0 2mm; font-weight: normal; }
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
  </style></head><body>
    <h1>Avtodom</h1>
    <h2>Масляная книжка</h2>
    <div class="plate">${esc(b.plate)}${b.brand || b.model ? ` · ${esc([b.brand, b.model].filter(Boolean).join(' '))}` : ''}</div>
    ${opts.client ? `<div class="row"><span>Владелец</span><b>${esc(opts.client)}</b></div>` : ''}
    ${
      last
        ? `<div class="row"><span>Замена</span><b>${esc(formatDate(last.change_date))}</b></div>
    ${recordMileage(last) !== null ? `<div class="row"><span>Пробег</span><b>${esc(recordMileage(last) ?? '')}</b></div>` : ''}
    ${recordLines(last)
      .map((l) => `<div class="row"><span>${esc(slotLabel(l.slot))}</span><b>${esc(l.text)}</b></div>`)
      .join('')}
    ${opts.master ? `<div class="row"><span>Мастер</span><b>${esc(opts.master)}</b></div>` : ''}
    ${last.declined?.trim() ? `<div class="row"><span>Рекомендовано</span><b>${esc(last.declined.trim())}</b></div>` : ''}`
        : '<div class="muted">Замен пока не было</div>'
    }
    ${next ? `<div class="next">Следующая замена — что наступит раньше${next}</div>` : ''}
    ${before ? `<div class="muted">По прошлым заменам</div>${before}` : ''}
    ${history ? `<div class="muted">Прошлые замены</div><table>${history}</table>` : ''}
    ${
      qr
        ? `<div class="qr">${qr}</div><div class="muted">Сканируйте камерой телефона: история замен,<br>напоминание о замене и бонусные баллы в Телеграм</div>`
        : ''
    }
  </body></html>`
}

/** Печать на чековом принтере через диалог браузера; `saleId` — после этого чека (ADR-111). */
export async function printOilBook(b: VehicleBook, opts: { client?: string | null; master?: string | null; saleId?: string | null } = {}): Promise<void> {
  printInFrame(await oilBookHtml(b, { ...opts, link: await botLink() }))
}
