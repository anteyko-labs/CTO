// «Настройки → Отчёт о прибыли» (задача 003 Н12): период «Прибыли и убытков» по умолчанию, поле
// «Кому» в расходе и показ сторно. Только владелец; каждое изменение — в журнал (сервер).
import { Link } from 'react-router-dom'
import { Card, CardTitle, ErrorBox, Loading, PageHeader, toast } from '../components/ui'
import { get, put } from '../lib/api'
import { useAction, useLoad } from '../lib/hooks'
import type { ProfitSettings as Settings } from '../lib/types'

const PERIODS: [Settings['default_period'], string][] = [
  ['month', 'Этот месяц (с 1-го числа по сегодня)'],
  ['today', 'Сегодня'],
  ['week', '7 дней'],
]

const PAYEE: [Settings['expense_payee'], string, string][] = [
  ['optional', 'Необязательно', 'Поле есть, можно оставить пустым'],
  ['required', 'Обязательно', 'Без «Кому» расход не записать — ни в программе, ни в боте'],
  ['off', 'Не спрашивать', 'Поля «Кому» в форме нет'],
]

export default function ProfitSettings() {
  const st = useLoad(() => get<Settings>('/settings/profit-report'), [])
  const act = useAction()
  const s = st.data

  const save = (next: Settings, done: string) =>
    void act.run(async () => {
      st.setData(next)
      try {
        st.setData(await put<Settings>('/settings/profit-report', next))
        toast(done)
      } catch (e) {
        st.reload()
        throw e
      }
    })

  const radio = <T extends string>(name: string, value: T, current: T, label: string, onPick: () => void, hint?: string) => (
    <label key={value} className="flex items-start gap-2 text-sm">
      <input type="radio" name={name} className="mt-0.5 h-4 w-4" checked={current === value} disabled={act.busy} onChange={onPick} />
      <span>
        {label}
        {hint && <span className="block text-xs text-slate-500">{hint}</span>}
      </span>
    </label>
  )

  return (
    <div>
      <PageHeader title="Отчёт о прибыли" subtitle="Как открывается «Прибыль и убытки» и что спрашивать в расходе" back={<Link to="/settings">← Настройки</Link>} />
      <ErrorBox error={st.error ?? act.error} />
      {!s ? (
        <Loading />
      ) : (
        <div className="space-y-4">
          <Card data-testid="profit-settings-period">
            <CardTitle>Период при открытии</CardTitle>
            <div className="flex flex-col gap-2" role="radiogroup" aria-label="Период при открытии">
              {PERIODS.map(([v, label]) => radio('profit-period', v, s.default_period, label, () => save({ ...s, default_period: v }, `Период по умолчанию: ${label.toLowerCase()}`)))}
            </div>
          </Card>
          <Card data-testid="profit-settings-payee">
            <CardTitle>Поле «Кому» в расходе</CardTitle>
            <div className="flex flex-col gap-2" role="radiogroup" aria-label="Поле «Кому» в расходе">
              {PAYEE.map(([v, label, hint]) => radio('profit-payee', v, s.expense_payee, label, () => save({ ...s, expense_payee: v }, `«Кому»: ${label.toLowerCase()}`), hint))}
            </div>
            <p className="mt-2 text-sm text-slate-500">
              «Кому» видно в списке расходов и в раскрытии статьи отчёта. Оплату работникам проводите выплатой в «Расчёте» — она уже входит в «Зарплату
              работникам».
            </p>
          </Card>
          <Card>
            <CardTitle>Сторно и исправления</CardTitle>
            <label className="inline-flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                className="h-4 w-4"
                checked={s.show_reversals}
                disabled={act.busy}
                onChange={(e) => save({ ...s, show_reversals: e.target.checked }, e.target.checked ? 'Сторно показываются сразу' : 'Сторно скрыты, пока не включите')}
              />
              Показывать сторно и исправления в раскрытии сразу
            </label>
            <p className="mt-2 text-sm text-slate-500">Пара «расход и его сторно» гасится в статье — итог от показа не меняется.</p>
          </Card>
        </div>
      )}
    </div>
  )
}
