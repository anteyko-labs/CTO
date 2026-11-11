// Печатный акт приёмки товара (задача 035): по желанию, у проведённой приёмки. A4, два экземпляра
// (точке и поставщику), внизу «Сдал» — от руки, «Принял» — кассир с подписью с планшета.
import { reasonLabel, qtyWords, type Acceptance } from './acceptances'
import { esc, pngDataUrl, printHtml, signatureSlot, type DebtDocSettings, type Seller } from './debtDocs'
import { get } from './api'
import { formatSomExact } from './format'

/** Дата по Бишкеку — дд.мм.гггг. */
function dateRu(iso: string | null): string {
  if (!iso) return '«___» __________ 20___ г.'
  const d = new Date(iso)
  return new Intl.DateTimeFormat('ru-RU', { timeZone: 'Asia/Bishkek', day: '2-digit', month: '2-digit', year: 'numeric' }).format(d)
}

/** Один экземпляр акта. Цены и суммы — только если сервер их отдал (право «Приход товара»). */
export function acceptanceActHtml(a: Acceptance, seller: Seller, signature: string | null): string {
  const money = a.total_tyiyn !== null
  const date = dateRu(a.closed_at)
  const rows = a.lines
    .map((l, i) => {
      const got = l.accepted_qty
      const diff = got === null ? '' : got === l.invoice_qty ? '—' : `${got > l.invoice_qty ? '+' : '−'}${qtyWords(l, Math.abs(got - l.invoice_qty))}`
      const why = [l.accepted_name ? `принят вместо: ${l.accepted_name}` : '', l.reason ? reasonLabel(l.reason) : '', l.comment].filter(Boolean).join('; ')
      const price = money && l.line_cost_tyiyn !== null && l.invoice_qty > 0 ? formatSomExact(Math.round(l.line_cost_tyiyn / l.invoice_qty)) : ''
      return `<tr><td class="c">${i + 1}</td><td>${esc(l.name)}</td><td>${esc(l.article)}</td>
        <td class="r">${esc(qtyWords(l, l.invoice_qty))}</td><td class="r">${got === null ? '—' : esc(qtyWords(l, got))}</td>
        <td class="r">${esc(diff)}</td><td>${esc(why)}</td>${money ? `<td class="r">${esc(price)}</td><td class="r">${esc(formatSomExact(l.line_cost_tyiyn ?? 0))}</td>` : ''}</tr>`
    })
    .join('')
  const total = money ? `<tr><td colspan="8" class="r"><b>Итого по накладной</b></td><td class="r"><b>${esc(formatSomExact(a.total_tyiyn ?? 0))}</b></td></tr>` : ''
  const extras =
    a.extras.length > 0
      ? `<p><b>Привезли сверх накладной (в приход не вошло):</b> ${esc(a.extras.map((x) => `${x.name || x.barcode} — ${x.qty} шт`).join('; '))}</p>`
      : ''
  const comment = a.accept_comment ? `<p>Комментарий: ${esc(a.accept_comment)}</p>` : ''
  const receiver = a.employee_name ?? a.closed_by_name ?? ''
  return `<div class="copy"><div class="doc">
    <h2 style="text-align:center">Акт приёмки товара № ${a.receipt_number ?? ''} от ${esc(date)}</h2>
    <p>Получатель: ${esc(seller.name)}${seller.inn ? `, ИНН ${esc(seller.inn)}` : ''}${seller.address ? `, ${esc(seller.address)}` : ''}</p>
    <p>Поставщик: ${esc(a.supplier_name)}${a.supplier_doc ? ` · накладная № ${esc(a.supplier_doc)}` : ''}</p>
    <table><thead><tr><th>№</th><th>Товар</th><th>Артикул</th><th>По накладной</th><th>Принято</th><th>Расхождение</th><th>Причина</th>${money ? '<th>Цена</th><th>Сумма</th>' : ''}</tr></thead>
    <tbody>${rows}${total}</tbody></table>
    ${extras}${comment}
    <div class="sign">
      <div>Сдал (поставщик / водитель):<br><br>${signatureSlot(null)} / ________________ /<br><br>Дата: «___» __________ 20___ г.</div>
      <div>Принял: ${esc(receiver)}<br><br>${signatureSlot(signature)} / ${esc(receiver) || '________________'} /<br><br>Дата: ${esc(date)}</div>
    </div>
  </div></div>`
}

/** Печать акта: реквизиты точки и подпись кассира с сервера, два экземпляра. */
export async function printAcceptanceAct(a: Acceptance, copies = 2): Promise<void> {
  const [settings, signature] = await Promise.all([
    get<DebtDocSettings>('/settings/debt-docs').catch(() => null),
    a.has_signature ? pngDataUrl(`/acceptances/${a.id}/signature`) : Promise.resolve(null),
  ])
  const seller: Seller = settings?.seller ?? { name: '', inn: '', address: '', phone: '', director: '', bank: '', city: '' }
  const one = acceptanceActHtml(a, seller, signature)
  printHtml(`Акт приёмки № ${a.receipt_number ?? ''}`, Array.from({ length: copies }, () => one).join(''))
}
