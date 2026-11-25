// Долг фирмы по видам (задача 051, ADR-215): под общим долгом — за машины фирмы и за личные
// машины работников. Нет долга за личные машины — ничего.
import { KIND_COMPANY, KIND_PERSONAL, debtKinds } from '../lib/debtDocs'
import { formatSom } from '../lib/format'

export function DebtKindsLine({ balance, personal, className = 'text-xs text-slate-500' }: { balance: number; personal?: number; className?: string }) {
  const k = debtKinds(balance, personal)
  if (!k) return null
  return (
    <div className={className} data-testid="debt-kinds">
      {KIND_COMPANY} {formatSom(k.company)} · {KIND_PERSONAL} {formatSom(k.personal)}
    </div>
  )
}
