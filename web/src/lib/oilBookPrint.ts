// Печать масляной книжки на ленте 80 мм: последняя замена, следующая и QR-код на Телеграм-бот (SPEC-16).
import QRCode from 'qrcode'
import type { VehicleBook } from '../components/OilBook'
import { get } from './api'
import { formatDate } from './format'

const esc = (s: string) =>
  s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c] ?? c)

const km = (v: number) => `${v.toLocaleString('ru-RU').replace(/\s/g, ' ')} км`

/** Ссылка на бота, если он подключён на сервере. */
export async function botLink(): Promise<string | null> {
  const r = await get<{ username: string | null }>('/telegram/bot').catch(() => ({ username: null }))
  return r.username ? `https://t.me/${r.username}` : null
}

export async function oilBookHtml(b: VehicleBook, opts: { client?: string | null; master?: string | null; link: string | null }): Promise<string> {
  const last = b.records[0]
  const next = [b.next_km !== null ? `на ${km(b.next_km)}` : '', b.next_date ? `до ${formatDate(b.next_date)}` : ''].filter(Boolean).join(' или ')
  const qr = opts.link ? await QRCode.toString(opts.link, { type: 'svg', margin: 0, errorCorrectionLevel: 'M' }) : ''
  const history = b.records
    .slice(1, 4)
    .map(
      (r) =>
        `<tr><td>${esc(formatDate(r.change_date))}</td><td>${r.mileage_km !== null ? esc(km(r.mileage_km)) : ''}</td><td>${esc(r.oil_text || r.filter_text)}</td></tr>`,
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
    .next b { font-size: 12pt; display: block; margin-top: 1mm; }
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
    ${last.mileage_km !== null ? `<div class="row"><span>Пробег</span><b>${esc(km(last.mileage_km))}</b></div>` : ''}
    ${last.oil_text ? `<div class="row"><span>Масло</span><b>${esc(last.oil_text)}</b></div>` : ''}
    ${last.filter_text ? `<div class="row"><span>Фильтр</span><b>${esc(last.filter_text)}</b></div>` : ''}
    ${opts.master ? `<div class="row"><span>Мастер</span><b>${esc(opts.master)}</b></div>` : ''}`
        : '<div class="muted">Замен пока не было</div>'
    }
    ${next ? `<div class="next">Следующая замена<b>${esc(next)}</b><span class="muted">что наступит раньше</span></div>` : ''}
    ${history ? `<div class="muted">Прошлые замены</div><table>${history}</table>` : ''}
    ${
      qr
        ? `<div class="qr">${qr}</div><div class="muted">Сканируйте камерой телефона: история замен,<br>напоминание о замене и бонусные баллы в Телеграм</div>`
        : ''
    }
  </body></html>`
}

/** Печать на чековом принтере через диалог браузера. */
export async function printOilBook(b: VehicleBook, opts: { client?: string | null; master?: string | null } = {}): Promise<void> {
  const html = await oilBookHtml(b, { ...opts, link: await botLink() })
  const frame = document.createElement('iframe')
  Object.assign(frame.style, { position: 'fixed', width: '0', height: '0', border: '0' })
  document.body.appendChild(frame)
  const doc = frame.contentDocument
  const win = frame.contentWindow
  if (!doc || !win) {
    frame.remove()
    return
  }
  doc.open()
  doc.write(html)
  doc.close()
  win.focus()
  win.print()
  setTimeout(() => frame.remove(), 60_000)
}
