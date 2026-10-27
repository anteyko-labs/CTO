// Чек смены 80 мм (SPEC-36 §10, ADR-120): печать через окно браузера, как товарный чек. Данные —
// отчёт смены `GET /shifts/{id}/report`; себестоимость и прибыль сервер отдаёт только владельцу,
// и шаблон показывает их только при `owner_view` (инвариант 6).
import { get } from '../lib/api'
import { branchLabel } from '../lib/branch'
import { formatDateTime, formatSomExact } from '../lib/format'
import { printInFrame } from '../lib/printFrame'
import { bankParts, bankTotals, debtTotals, methodName, type ShiftReport } from '../lib/shiftReport'

const esc = (s: string) =>
  s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c] ?? c)

const som = (v: number) => esc(formatSomExact(v))

/** Строка «подпись … сумма»; `sub` — с отступом (`deep` — с двойным), `strong` — итог. */
function row(label: string, value: number, opts: { sub?: boolean; deep?: boolean; strong?: boolean; sign?: boolean } = {}): string {
  const text = opts.sign && value > 0 ? `+${som(value)}` : som(value)
  const cls = [opts.deep ? 'sub2' : opts.sub ? 'sub' : '', opts.strong ? 'strong' : ''].filter(Boolean).join(' ')
  return `<tr${cls ? ` class="${cls}"` : ''}><td>${esc(label)}</td><td class="r">${text}</td></tr>`
}

const section = (title: string, rows: string) => (rows ? `<h2>${esc(title)}</h2><table>${rows}</table>` : '')

/** HTML чека смены. */
export function shiftHtml(r: ShiftReport): string {
  const s = r.shift
  const c = r.cash
  const v = r.revenue
  const head = [
    `<div>Касса: ${esc(s.account_name)}</div>`,
    `<div>Кассир: ${esc(s.cashier_name)}</div>`,
    `<div>Открыта: ${esc(formatDateTime(s.opened_at))}, ${esc(s.opened_by)}</div>`,
    s.closed_at
      ? `<div>Закрыта: ${esc(formatDateTime(s.closed_at))}${s.closed_by ? `, ${esc(s.closed_by)}` : ''}</div>`
      : '<div>Смена ещё открыта</div>',
  ].join('')

  const revenue =
    row(`Продажи · ${v.sales_count} чек.`, v.sales_tyiyn) +
    (v.returns_tyiyn !== 0 ? row(`Возвраты · ${v.returns_count}`, -v.returns_tyiyn) : '') +
    row('Выручка', v.net_tyiyn, { strong: true }) +
    v.methods.map((m) => row(methodName(m.method), m.amount_tyiyn, { sub: true })).join('') +
    (v.rest_tyiyn !== 0 ? row('Исправления и прочее', v.rest_tyiyn, { sub: true }) : '')

  const banks = s.bank_split ?? []
  const all = bankTotals(banks)
  const bank =
    banks
      .map(
        (b) =>
          row(`«${b.name}»`, b.total_tyiyn, { strong: true }) +
          bankParts(b)
            .map((p) => row(p.label, p.value, { sub: true }))
            .join('') +
          (b.fee_tyiyn !== 0 ? row('Комиссия банка', -b.fee_tyiyn, { sub: true }) : '') +
          row('Упало на счёт', b.landed_tyiyn ?? b.total_tyiyn - b.fee_tyiyn, { sub: true }),
      )
      .join('') +
    (banks.length > 1 ? row('Весь безнал смены', all.total, { strong: true }) + row('Упало на счета', all.landed, { sub: true }) : '') +
    (banks.length === 0 ? '<tr><td colspan="2">Безналичных счетов нет</td></tr>' : '')

  // Выплаты — кому сколько, под строкой «Выплаты сотрудникам».
  const payouts = r.payouts.map((p) => row(`${p.employee_name}${p.reversal ? ' (сторно)' : ''}`, p.amount_tyiyn, { deep: true })).join('')
  const outgo = c.outgo
    .filter((l) => l.amount_tyiyn !== 0)
    .map((l) => row(l.label, l.amount_tyiyn, { sub: true }) + (l.key === 'payouts' ? payouts : ''))
    .join('')
  const diff = c.diff_tyiyn
  const cash =
    row('На начало смены', c.opening_tyiyn) +
    row('Приход', c.income_tyiyn) +
    c.income
      .filter((l) => l.amount_tyiyn !== 0)
      .map((l) => row(l.label, l.amount_tyiyn, { sub: true }))
      .join('') +
    row('Расход', c.outgo_tyiyn) +
    outgo +
    row('Должно быть', c.expected_tyiyn, { strong: true }) +
    (c.counted_tyiyn !== null ? row('Насчитали', c.counted_tyiyn) : '') +
    (diff !== null ? (diff === 0 ? '<tr><td>Разница</td><td class="r">сошлось</td></tr>' : row(diff < 0 ? 'Недостача' : 'Излишек', diff, { sign: true })) : '') +
    (c.handover_tyiyn !== null
      ? (c.handover_tyiyn > 0 ? row(`Сдано${c.handover_to ? ` в «${c.handover_to}»` : ''}`, c.handover_tyiyn) : '') + row('Осталось в кассе', c.left_tyiyn ?? 0)
      : '')

  const d = debtTotals(r.debts)
  const debts = (d.sold !== 0 ? row('Дали в долг', d.sold) : '') + (d.repaid !== 0 ? row('Погасили', d.repaid) : '') + (d.adjusted !== 0 ? row('Правки долга', d.adjusted) : '')

  const cancelled =
    r.cancelled.length > 0
      ? row(`Отменено строк: ${r.cancelled.length}`, r.cancelled_tyiyn) +
        r.cancelled
          .slice(0, 15)
          .map((x) => row(x.name, x.amount_tyiyn, { sub: true }))
          .join('') +
        (r.cancelled.length > 15 ? `<tr class="sub"><td colspan="2">ещё ${r.cancelled.length - 15}</td></tr>` : '')
      : ''

  // Прибыль — только владельцу: администратору сервер её не присылает, и шаблон её не печатает.
  const owner = r.owner_view && v.gross_tyiyn !== undefined
  const profit = owner
    ? row('Себестоимость', v.cost_tyiyn ?? 0) +
      row('Валовая прибыль', v.gross_tyiyn ?? 0, { strong: true }) +
      (v.profit ?? [])
        .filter((l) => l.key !== 'gross')
        .map((l) => row(l.label, l.amount_tyiyn, { sub: true, sign: true }))
        .join('') +
      (v.net_profit_tyiyn !== undefined ? row('Чистая прибыль', v.net_profit_tyiyn, { strong: true }) : '')
    : ''

  return `<!doctype html><html><head><meta charset="utf-8"><title>Чек смены ${s.number}</title><style>
    @page { size: 80mm auto; margin: 3mm; }
    body { font-family: Arial, sans-serif; font-size: 10pt; margin: 0; width: 72mm; }
    h1 { font-size: 12pt; text-align: center; margin: 0 0 1mm; }
    h2 { font-size: 10pt; margin: 3mm 0 1mm; border-top: 1px dashed #000; padding-top: 1mm; }
    table { width: 100%; border-collapse: collapse; }
    td { padding: 0.3mm 0; vertical-align: top; }
    .r { text-align: right; white-space: nowrap; padding-left: 2mm; }
    .sub td:first-child { padding-left: 3mm; color: #333; }
    .sub2 td:first-child { padding-left: 6mm; color: #333; }
    .strong td { font-weight: bold; }
    .center { text-align: center; }
    .note { font-size: 8pt; color: #333; margin-top: 1mm; }
  </style></head><body>
    <h1>Avtodom</h1>
    ${branchLabel() ? `<div class="center">${esc(branchLabel() ?? '')}</div>` : ''}
    <div class="center"><b>Чек смены № ${s.number}</b></div>
    ${head}
    ${section('Выручка', revenue)}
    ${section('Безнал по счетам', bank)}
    ${section('Деньги в кассе', cash)}
    ${section('Долги покупателей', debts)}
    ${section('Отменённые товары', cancelled)}
    ${section('Прибыль', profit)}
    ${owner ? '<div class="note">Чистая — без оклада: оклад начисляется в конце месяца. Чеки вне смен в смены не входят.</div>' : ''}
    <div class="center note">Напечатано ${esc(formatDateTime(new Date().toISOString()))}</div>
  </body></html>`
}

/** Напечатать чек смены: свежий отчёт с сервера. */
export async function printShift(id: string): Promise<void> {
  const r = await get<ShiftReport>(`/shifts/${id}/report`)
  printInFrame(shiftHtml(r))
}
