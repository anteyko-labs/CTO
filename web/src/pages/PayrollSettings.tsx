// «Настройки → Зарплата» (задача 003 Н11): дата выплаты задним числом, выплаты без открытой смены,
// «заработано на сегодня» во вкладке «Оклад», напоминание в день расчёта. Только владелец, каждое
// изменение — в журнал (сервер). По умолчанию: 35 дней, без смены можно, ориентира нет, напоминать.
import { useState } from 'react'
import { Link } from 'react-router-dom'
import { Button, Card, Checkbox, ErrorBox, Field, Loading, PageHeader, toast } from '../components/ui'
import { get, put } from '../lib/api'
import { useAction, useLoad } from '../lib/hooks'
import type { PayrollSettings as Settings } from '../lib/types'

export default function PayrollSettings() {
  const st = useLoad(() => get<Settings>('/settings/payroll'), [])
  const [draft, setDraft] = useState<Settings | null>(null)
  const act = useAction()
  const s = draft ?? st.data
  const set = (patch: Partial<Settings>) => s && setDraft({ ...s, ...patch })
  const [days, setDays] = useState<string | null>(null)
  const daysText = days ?? String(s?.payout_backdate_days ?? '')

  const save = () =>
    void act.run(async () => {
      if (!s) return
      const n = Number(daysText.trim())
      if (!Number.isInteger(n) || n < 0 || n > 366) throw new Error('Дней назад — целое число от 0 до 366')
      const saved = await put<Settings>('/settings/payroll', { ...s, payout_backdate_days: n })
      st.setData(saved)
      setDraft(null)
      setDays(null)
      toast('Настройки зарплаты сохранены')
    })

  return (
    <div>
      <PageHeader title="Зарплата" subtitle="Выплаты и оклад сотрудникам" back={<Link to="/settings">← Настройки</Link>} />
      <ErrorBox error={st.error ?? act.error} />
      {!s ? (
        <Loading />
      ) : (
        <Card className="flex max-w-2xl flex-col gap-4" data-testid="payroll-settings">
          <Field
            label="Дату выплаты можно поставить задним числом, дней"
            hint="Владелец выбирает, каким днём выплата видна в расчёте; деньги всё равно уходят сейчас. 0 — только сегодня."
          >
            <input id="payroll-backdate" inputMode="numeric" className="w-28" value={daysText} onChange={(e) => setDays(e.target.value)} />
          </Field>
          <div className="flex flex-col gap-1">
            <Checkbox label="Выплаты только при открытой смене" checked={s.payout_requires_shift} onChange={(v) => set({ payout_requires_shift: v })} />
            <p className="text-sm text-slate-500">
              Выключено — без смены владелец платит из сейфа или со счёта, касса смены недоступна. Включено — без открытой смены никто не выплачивает
              ни с какого счёта. Выплаты при закрытии смены идут как обычно.
            </p>
          </div>
          <div className="flex flex-col gap-1">
            <Checkbox label="Показывать заработанное по окладу на сегодня" checked={s.salary_show_to_date} onChange={(v) => set({ salary_show_to_date: v })} />
            <p className="text-sm text-slate-500">Во вкладке «Оклад»: ставка × прошло дней / дней в периоде. Это ориентир, а не начисление.</p>
          </div>
          <div className="flex flex-col gap-1">
            <Checkbox label="В день расчёта оклада в 09:00 — напоминание" checked={s.salary_due_notify} onChange={(v) => set({ salary_due_notify: v })} />
            <p className="text-sm text-slate-500">
              «Сегодня расчёт оклада: … к выплате …» — в «Уведомлениях» и в боте (в боте выключается ещё и в «Уведомлениях в бот»). Оклад начисляет
              владелец сам.
            </p>
          </div>
          <div>
            <Button disabled={act.busy} onClick={save}>
              Сохранить
            </Button>
          </div>
        </Card>
      )}
    </div>
  )
}
