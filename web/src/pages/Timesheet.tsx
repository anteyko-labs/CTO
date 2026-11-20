// «Сотрудники → Табель» (задача 039 A, ADR-200): часы по дням за месяц по терминалу Face ID. Незакрытый к
// концу суток день — «не закрыт», без часов; владелец (или право «Табель») закрывает его временем с причиной.
import { useState } from 'react'
import { Link } from 'react-router-dom'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, Modal, PageHeader, toast } from '../components/ui'
import { get, post, qs } from '../lib/api'
import { bishkekMoment, bishkekTime, hours, type Timesheet as Sheet, type TimesheetDay, type TimesheetRow } from '../lib/faceid'
import { useAction, useLoad } from '../lib/hooks'

function thisMonth(): string {
  return new Date().toLocaleDateString('sv-SE', { timeZone: 'Asia/Bishkek' }).slice(0, 7)
}

export default function Timesheet() {
  const [month, setMonth] = useState(thisMonth())
  const sheet = useLoad(() => get<Sheet>(`/staff/timesheet${qs({ month })}`), [month])
  const [open, setOpen] = useState<{ row: TimesheetRow; day: TimesheetDay } | null>(null)
  const days = sheet.data ? Array.from({ length: sheet.data.days_in_month }, (_, i) => i + 1) : []
  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title="Табель"
        subtitle="Рабочий день — кнопкой в боте и лицом на терминале Face ID"
        back={
          <Link to="/employees" className="text-sm text-sky-700 hover:underline">
            ← Сотрудники
          </Link>
        }
        actions={
          <Link to="/settings/faceid" className="text-sm text-sky-700 hover:underline">
            Терминал Face ID
          </Link>
        }
      />
      <Field label="Месяц">
        <input id="timesheet-month" type="month" className="w-48" value={month} onChange={(e) => e.target.value && setMonth(e.target.value)} />
      </Field>
      <ErrorBox error={sheet.error} />
      {sheet.loading && !sheet.data && <Loading />}
      {sheet.data && sheet.data.rows.length === 0 && (
        <Empty icon="employees">За месяц часов нет. Номер сотрудника в терминале задаётся в его карточке.</Empty>
      )}
      {sheet.data && sheet.data.rows.length > 0 && (
        <Card className="overflow-x-auto p-0">
          <table className="min-w-full text-sm" data-testid="timesheet">
            <thead>
              <tr className="bg-slate-50 text-slate-500">
                <th className="sticky left-0 bg-slate-50 px-3 py-2 text-left">Сотрудник</th>
                {days.map((d) => (
                  <th key={d} className="px-1 py-2 text-center font-normal">
                    {d}
                  </th>
                ))}
                <th className="px-3 py-2 text-right">Итого</th>
              </tr>
            </thead>
            <tbody>
              {sheet.data.rows.map((row) => (
                <tr key={row.employee_id} className="border-t border-slate-100">
                  <td className="sticky left-0 whitespace-nowrap bg-white px-3 py-2 font-medium">{row.full_name}</td>
                  {days.map((d) => {
                    const date = `${sheet.data!.month}-${String(d).padStart(2, '0')}`
                    const day = row.days.find((x) => x.date === date)
                    return (
                      <td key={d} className="px-0.5 py-1 text-center">
                        {day && (
                          <button
                            type="button"
                            className={`rounded px-1 py-0.5 tabular-nums hover:bg-sky-50 ${day.unclosed ? 'bg-amber-100 text-amber-800' : ''}`}
                            title={day.unclosed ? 'не закрыт' : undefined}
                            onClick={() => setOpen({ row, day })}
                          >
                            {day.unclosed && day.minutes === 0 ? '?' : hours(day.minutes)}
                          </button>
                        )}
                      </td>
                    )
                  })}
                  <td className="px-3 py-2 text-right font-medium tabular-nums">{hours(row.total_minutes)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </Card>
      )}
      {open && (
        <DayModal
          row={open.row}
          day={open.day}
          onClose={() => setOpen(null)}
          onSaved={() => {
            setOpen(null)
            sheet.reload()
          }}
        />
      )}
    </div>
  )
}

function DayModal({ row, day, onClose, onSaved }: { row: TimesheetRow; day: TimesheetDay; onClose: () => void; onSaved: () => void }) {
  return (
    <Modal title={`${row.full_name}, ${new Date(day.date).toLocaleDateString('ru-RU')}`} onClose={onClose}>
      <div className="flex flex-col gap-3">
        {day.sessions.map((s) => (
          <div key={s.id} className="flex flex-col gap-2 rounded border border-slate-200 p-3">
            <div className="flex flex-wrap items-center gap-2">
              <span>Начало {bishkekTime(s.started_at)}</span>
              <span>· конец {s.ended_at ? bishkekTime(s.ended_at) : '—'}</span>
              {s.minutes !== null && <span className="text-slate-500">· {hours(s.minutes)}</span>}
              {s.status === 'unclosed' && <Badge tone="amber">не закрыт</Badge>}
              {s.status === 'open' && <Badge tone="sky">идёт</Badge>}
            </div>
            {s.close_reason && (
              <div className="text-sm text-slate-500">
                Закрыл {s.closed_by ?? 'владелец'}: {s.close_reason}
              </div>
            )}
            {s.status !== 'closed' && <CloseForm id={s.id} date={day.date} onSaved={onSaved} />}
          </div>
        ))}
      </div>
    </Modal>
  )
}

function CloseForm({ id, date, onSaved }: { id: string; date: string; onSaved: () => void }) {
  const [time, setTime] = useState('18:00')
  const [reason, setReason] = useState('')
  const act = useAction()
  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-end gap-2">
        <Field label="Конец дня">
          <input id="close-time" type="time" className="w-32" value={time} onChange={(e) => setTime(e.target.value)} />
        </Field>
        <Field label="Причина">
          <input id="close-reason" className="w-64" value={reason} onChange={(e) => setReason(e.target.value)} placeholder="забыл нажать «Закончить»" />
        </Field>
        <Button
          disabled={act.busy || !reason.trim() || !time}
          onClick={() =>
            void act.run(async () => {
              await post(`/staff/work-sessions/${id}/close`, { ended_at: bishkekMoment(date, time), reason: reason.trim() })
              toast('День закрыт')
              onSaved()
            })
          }
        >
          Закрыть день
        </Button>
      </div>
      <ErrorBox error={act.error} />
    </div>
  )
}
