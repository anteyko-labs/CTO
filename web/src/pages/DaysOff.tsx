// «Сотрудники → График» (задача 037 п.5, ADR-203): месяц, по строке на работника, клик — выходной.
// Выходной ниже минимума работающих сервер не ставит (409). Минимум задаёт владелец.
import { useState } from 'react'
import { Link } from 'react-router-dom'
import { Button, Card, Empty, ErrorBox, Field, Loading, PageHeader, toast } from '../components/ui'
import { get, newOpId, put, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { belowMinimum, dayOf, onDuty, type DaysOff as Sheet, type DaysOffEmployee } from '../lib/daysOff'
import { useAction, useLoad } from '../lib/hooks'

function thisMonth(): string {
  return new Date().toLocaleDateString('sv-SE', { timeZone: 'Asia/Bishkek' }).slice(0, 7)
}

export default function DaysOff() {
  const owner = useUser().role === 'owner'
  const [month, setMonth] = useState(thisMonth())
  const sheet = useLoad(() => get<Sheet>(`/staff/days-off${qs({ month })}`), [month])
  const act = useAction()
  const data = sheet.data
  const days = data ? Array.from({ length: data.days_in_month }, (_, i) => i + 1) : []

  const toggle = (e: DaysOffEmployee, date: string) =>
    void act.run(async () => {
      const off = !e.days_off.includes(date)
      await put('/staff/days-off', { op_id: newOpId(), employee_id: e.id, day: date, off })
      toast(off ? 'Выходной поставлен' : 'Выходной снят')
      sheet.reload()
    })

  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title="График выходных"
        subtitle="Клик по дню — выходной; в день должно остаться не меньше минимума работающих"
        back={
          <Link to="/employees" className="text-sm text-sky-700 hover:underline">
            ← Сотрудники
          </Link>
        }
      />
      <div className="flex flex-wrap items-end gap-4">
        <Field label="Месяц">
          <input id="days-off-month" type="month" className="w-48" value={month} onChange={(e) => e.target.value && setMonth(e.target.value)} />
        </Field>
        {data && owner && <MinForm value={data.min_on_duty} onSaved={sheet.reload} />}
        {data && !owner && <div className="text-sm text-slate-500">Минимум работающих: {data.min_on_duty || 'без ограничения'}</div>}
      </div>
      <ErrorBox error={sheet.error ?? act.error} />
      {sheet.loading && !data && <Loading />}
      {data && data.employees.length === 0 && <Empty icon="employees">Сотрудников нет.</Empty>}
      {data && data.employees.length > 0 && (
        <Card className="overflow-x-auto p-0">
          <table className="min-w-full text-sm" data-testid="days-off">
            <thead>
              <tr className="bg-slate-50 text-slate-500">
                <th className="sticky left-0 bg-slate-50 px-3 py-2 text-left">Сотрудник</th>
                {days.map((d) => (
                  <th key={d} className="px-1 py-2 text-center font-normal">
                    {d}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {data.employees.map((e) => (
                <tr key={e.id} className="border-t border-slate-100">
                  <td className="sticky left-0 whitespace-nowrap bg-white px-3 py-2 font-medium">{e.full_name}</td>
                  {days.map((d) => {
                    const date = dayOf(data.month, d)
                    const off = e.days_off.includes(date)
                    return (
                      <td key={d} className="px-0.5 py-1 text-center">
                        <button
                          type="button"
                          disabled={act.busy}
                          aria-label={`${e.full_name} ${date}`}
                          className={`h-7 w-7 rounded ${off ? 'bg-slate-700 text-white' : 'hover:bg-sky-50'}`}
                          onClick={() => toggle(e, date)}
                        >
                          {off ? 'В' : ''}
                        </button>
                      </td>
                    )
                  })}
                </tr>
              ))}
              <tr className="border-t border-slate-200 bg-slate-50 text-slate-500">
                <td className="sticky left-0 bg-slate-50 px-3 py-2">Работают</td>
                {days.map((d) => {
                  const date = dayOf(data.month, d)
                  return (
                    <td key={d} className={`px-0.5 py-1 text-center tabular-nums ${belowMinimum(data.employees, date, data.min_on_duty) ? 'text-red-600' : ''}`}>
                      {onDuty(data.employees, date)}
                    </td>
                  )
                })}
              </tr>
            </tbody>
          </table>
        </Card>
      )}
    </div>
  )
}

function MinForm({ value, onSaved }: { value: number; onSaved: () => void }) {
  const [min, setMin] = useState(String(value))
  const act = useAction()
  return (
    <div className="flex items-end gap-2">
      <Field label="Минимум работающих в день (0 — без ограничения)">
        <input id="min-on-duty" type="number" min={0} max={100} className="w-24" value={min} onChange={(e) => setMin(e.target.value)} />
      </Field>
      <Button
        variant="secondary"
        disabled={act.busy || min === String(value) || !/^\d+$/.test(min)}
        onClick={() =>
          void act.run(async () => {
            await put('/settings/staff', { op_id: newOpId(), min_on_duty: Number(min) })
            toast('Минимум сохранён')
            onSaved()
          })
        }
      >
        Сохранить
      </Button>
      <ErrorBox error={act.error} />
    </div>
  )
}
