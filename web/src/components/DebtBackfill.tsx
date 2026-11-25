// «Начислить процент по ждущим долгам» (задача 054): разовая кнопка владельца в «Настройки → Касса».
// Список чеков и итог считает сервер; начисление — датой каждого чека.
import { Button, ErrorBox, toast } from './ui'
import { get, newOpId, post } from '../lib/api'
import { byEmployee, checksText, confirmText, rateText, type Backfill } from '../lib/debtBackfill'
import { formatSom } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'

export function DebtBackfill({ settingOn }: { settingOn: boolean }) {
  const st = useLoad(() => get<Backfill>('/payroll/debt-backfill'), [])
  const act = useAction()
  const b = st.data
  if (st.error) return <ErrorBox error={st.error} />
  if (!b || b.lines.length === 0) return null
  return (
    <div className="mt-3 rounded-md border border-slate-200 p-3 text-sm" data-testid="debt-backfill">
      <div className="font-medium">
        Ждут погашения долга: {checksText(b.lines.length)}, процент кассирам — {formatSom(b.total_tyiyn)}
      </div>
      <ul className="mt-1 text-slate-600">
        {byEmployee(b.lines).map((e) => (
          <li key={e.name}>
            {e.name}: {formatSom(e.tyiyn)} ({checksText(e.count)})
          </li>
        ))}
      </ul>
      <details className="mt-1 text-xs text-slate-500">
        <summary className="cursor-pointer">Чеки</summary>
        <ul>
          {b.lines.map((l) => (
            <li key={l.sale_id}>
              № {l.sale_number}, {l.business_date.split('-').reverse().join('.')}, {l.party_name} — {l.employee_name},{' '}
              {rateText(l.rate_bp)} с {formatSom(l.base_tyiyn)} = {formatSom(l.percent_tyiyn)}
            </li>
          ))}
        </ul>
      </details>
      <div className="mt-2 flex flex-col items-start gap-1 sm:flex-row sm:items-center sm:gap-2">
        <Button
          variant="secondary"
          disabled={!settingOn || act.busy}
          onClick={() => {
            if (!window.confirm(confirmText(b))) return
            void act.run(async () => {
              const r = await post<Backfill>('/payroll/debt-backfill', { op_id: newOpId() })
              toast(`Начислено ${formatSom(r.total_tyiyn)} по ${r.lines.length} чекам`)
              st.reload()
            })
          }}
        >
          Начислить процент по ждущим долгам
        </Button>
        {!settingOn && <span className="text-xs text-slate-500">Сначала включите и сохраните «Процент кассира с долга — сразу».</span>}
      </div>
      <ErrorBox error={act.error} />
    </div>
  )
}
