// Безнал смены по счетам (SPEC-36 §9, ADR-120): виден всегда, даже при одном счёте. По каждому
// безналичному счёту — чеки по способам, оплаты долга, возвраты, комиссия и сколько упало на счёт;
// внизу при нескольких счетах — весь безнал смены. Суммы считает сервер по движениям счетов.
import { formatSom } from '../lib/format'
import { bankParts, bankTotals } from '../lib/shiftReport'
import type { BankSplit } from '../lib/types'

function Line({ label, value, strong, sub }: { label: string; value: number; strong?: boolean; sub?: boolean }) {
  return (
    <div className={`flex items-baseline justify-between gap-3 py-0.5 ${sub ? 'pl-3 text-slate-600' : ''} ${strong ? 'font-semibold' : ''}`}>
      <span className="min-w-0">{label}</span>
      <span className={`whitespace-nowrap tabular-nums ${value < 0 ? 'text-rose-700' : ''}`}>{formatSom(value)}</span>
    </div>
  )
}

export function ShiftBank({ list, testId = 'shift-bank' }: { list: BankSplit[]; testId?: string }) {
  const all = bankTotals(list)
  return (
    <div className="text-sm" data-testid={testId}>
      <div className="mb-1 text-xs font-medium tracking-wide text-slate-500 uppercase">Безнал по счетам</div>
      {list.length === 0 && <div className="py-1 text-slate-500">Безналичных счетов нет</div>}
      {list.map((b) => (
        <div key={b.account_id} className="mb-2 rounded-md border border-slate-200 px-3 py-1.5" data-testid="bank-account">
          <Line label={`«${b.name}»`} value={b.total_tyiyn} strong />
          {bankParts(b).map((p) => (
            <Line key={p.key} label={p.label} value={p.value} sub />
          ))}
          {b.fee_tyiyn !== 0 && <Line label="Комиссия банка" value={-b.fee_tyiyn} sub />}
          <div data-testid="bank-landed">
            <Line label="Упало на счёт" value={b.landed_tyiyn ?? b.total_tyiyn - b.fee_tyiyn} />
          </div>
        </div>
      ))}
      {list.length > 1 && (
        <div className="rounded-md bg-slate-50 px-3 py-1.5" data-testid="bank-all">
          <Line label="Весь безнал смены" value={all.total} strong />
          {all.fee !== 0 && <Line label="Комиссия банка" value={-all.fee} sub />}
          <Line label="Упало на счета" value={all.landed} />
        </div>
      )}
    </div>
  )
}
