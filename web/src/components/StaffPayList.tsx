// Кто сколько заработал: понятные строки по видам начислений (SPEC-07).
import { formatSom } from '../lib/format'
import type { StaffPay } from '../lib/types'

const KIND: Record<string, (count: number) => string> = {
  service_fee: (n) => `замены ×${n}`,
  revenue_percent: () => '% с чеков',
  shift_fee: () => 'за смену',
  monthly_salary: () => 'оклад',
  bonus: () => 'премия',
  penalty: () => 'удержание',
  shortage: () => 'недостача',
}

export function StaffPayList({ staff, empty = 'Пока никому не начислено' }: { staff: StaffPay[]; empty?: string }) {
  if (staff.length === 0) return <div className="text-sm text-slate-500">{empty}</div>
  return (
    <ul className="divide-y divide-slate-100">
      {staff.map((p) => (
        <li key={p.employee_id} className="grid grid-cols-[minmax(0,1fr)_auto] items-baseline gap-3 py-2">
          <div className="min-w-0">
            <div className="font-medium">{p.name}</div>
            <div className="text-xs text-slate-500">
              {p.items.map((i) => `${(KIND[i.kind] ?? (() => i.kind))(i.count)} — ${formatSom(i.amount_tyiyn)}`).join(' · ')}
            </div>
          </div>
          <div className="whitespace-nowrap text-right">
            <div className={`text-lg font-semibold ${p.total_tyiyn < 0 ? 'text-rose-700' : ''}`}>{formatSom(p.total_tyiyn)}</div>
            <div className="text-xs text-slate-500">
              {(p.paid_tyiyn ?? 0) !== 0 && <>выдано {formatSom(p.paid_tyiyn ?? 0)} · </>}
              <span className={(p.owed_tyiyn ?? 0) > 0 ? 'font-medium text-amber-700' : ''}>
                {(p.owed_tyiyn ?? 0) > 0 ? `к выдаче ${formatSom(p.owed_tyiyn ?? 0)}` : (p.owed_tyiyn ?? 0) < 0 ? `аванс ${formatSom(-(p.owed_tyiyn ?? 0))}` : 'всё выдано'}
              </span>
            </div>
          </div>
        </li>
      ))}
    </ul>
  )
}
