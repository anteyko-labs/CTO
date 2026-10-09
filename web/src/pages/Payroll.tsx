import { useState } from 'react'
import { PayrollMonth } from '../components/PayrollMonth'
import { Button, Card, Empty, ErrorBox, Field, Loading, Modal, PageHeader, Table, toast } from '../components/ui'
import { get, newOpId, post, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatSom, parseSom, somInput, todayBishkek } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import type { PayrollRow } from '../lib/types'

/** Расчёт за день: кто сколько заработал и сколько ему должны (SPEC-07). */
export default function Payroll() {
  const owner = useUser().role === 'owner'
  const [view, setView] = useState<'day' | 'month'>('day')
  const [date, setDate] = useState(todayBishkek())
  const [paying, setPaying] = useState<PayrollRow | null>(null)
  const [extra, setExtra] = useState<{ row: PayrollRow; kind: 'bonus' | 'penalty' } | null>(null)
  const [salary, setSalary] = useState<PayrollRow | null>(null)
  const [form, setForm] = useState({ sum: '', comment: '', advance: false })
  // Один op_id на открытую форму: повтор после потерянного ответа не задвоит операцию.
  const [opId, setOpId] = useState(newOpId)
  const act = useAction()
  const rows = useLoad(() => get<PayrollRow[]>(`/payroll/day${qs({ date })}`), [date])
  const overpay = paying !== null && (parseSom(form.sum) ?? 0) > paying.balance_tyiyn

  const close = () => {
    setPaying(null)
    setExtra(null)
    setSalary(null)
    setForm({ sum: '', comment: '', advance: false })
    setOpId(newOpId())
  }

  const pay = () =>
    void act.run(async () => {
      if (!paying) return
      const sum = parseSom(form.sum)
      if (sum === null || sum <= 0) throw new Error('Неверная сумма')
      await post('/payouts', {
        op_id: opId,
        employee_id: paying.employee_id,
        amount_tyiyn: sum,
        source: 'account',
        // Аванс сверх заработанного — только с явной отметки владельца (SPEC-07).
        advance: sum > paying.balance_tyiyn && form.advance,
        comment: form.comment,
      })
      close()
      rows.reload()
      toast('Выплачено из кассы')
    })

  const addExtra = () =>
    void act.run(async () => {
      if (!extra) return
      const sum = parseSom(form.sum)
      if (sum === null || sum <= 0) throw new Error('Неверная сумма')
      await post('/payroll/accruals', {
        op_id: opId,
        employee_id: extra.row.employee_id,
        kind: extra.kind,
        amount_tyiyn: sum,
        business_date: date,
        comment: form.comment,
      })
      close()
      rows.reload()
      toast(extra.kind === 'bonus' ? 'Бонус начислен' : 'Удержание записано')
    })

  const payMonth = () =>
    void act.run(async () => {
      if (!salary) return
      const sum = form.sum.trim() ? parseSom(form.sum) : null
      if (form.sum.trim() && sum === null) throw new Error('Неверная сумма')
      await post('/payroll/salary', {
        op_id: opId,
        employee_id: salary.employee_id,
        month: `${date.slice(0, 7)}-01`,
        amount_tyiyn: sum,
      })
      close()
      rows.reload()
      toast('Оклад начислен')
    })

  const list = rows.data ?? []
  const toPay = list.reduce((acc, r) => acc + Math.max(0, r.balance_tyiyn), 0)

  return (
    <div>
      <PageHeader title="Расчёт сотрудников" />
      <div className="mb-4 inline-grid grid-cols-2 overflow-hidden rounded-md border border-slate-300 text-sm">
        {(
          [
            ['day', 'За день'],
            ['month', 'За месяц'],
          ] as const
        ).map(([v, label]) => (
          <button
            key={v}
            type="button"
            className={`min-h-[42px] px-4 py-2 ${view === v ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
            onClick={() => setView(v)}
          >
            {label}
          </button>
        ))}
      </div>
      {view === 'month' ? (
        <PayrollMonth />
      ) : (
      <>
      <Card className="mb-4 flex flex-wrap items-end gap-4">
        <Field label="День">
          <input type="date" className="w-44" value={date} onChange={(e) => setDate(e.target.value)} />
        </Field>
        <div className="ml-auto text-right">
          <div className="text-xs text-slate-500">К выплате всего</div>
          <div className="text-2xl font-bold">{formatSom(toPay)}</div>
        </div>
      </Card>

      <Card>
        <ErrorBox error={rows.error ?? act.error} />
        {rows.loading && !rows.data ? (
          <Loading />
        ) : list.length === 0 ? (
          <Empty>За этот день никто ничего не заработал</Empty>
        ) : (
          <Table head={['Сотрудник', 'Долг на начало', 'За замены', 'Процент', 'Прочее', 'Выплачено', 'К выплате', '']}>
            {list.map((r) => (
              <tr key={r.employee_id}>
                <td className="px-2 py-2 font-medium">{r.full_name}</td>
                <td className="whitespace-nowrap px-2 py-2">{formatSom(r.opening_tyiyn)}</td>
                <td className="whitespace-nowrap px-2 py-2">{formatSom(r.service_fee_tyiyn)}</td>
                <td className="whitespace-nowrap px-2 py-2">
                  {formatSom(r.percent_tyiyn)}
                  {owner && r.base_tyiyn !== null && r.base_tyiyn !== 0 && (
                    <div className="text-xs text-slate-500">с валовой {formatSom(r.base_tyiyn)}</div>
                  )}
                </td>
                <td className="whitespace-nowrap px-2 py-2">{formatSom(r.other_tyiyn)}</td>
                <td className="whitespace-nowrap px-2 py-2">{formatSom(r.paid_tyiyn)}</td>
                <td className="whitespace-nowrap px-2 py-2 font-semibold">{formatSom(r.balance_tyiyn)}</td>
                <td className="px-2 py-2 text-right">
                  <div className="flex flex-wrap justify-end gap-1">
                    {owner && (
                      <>
                        <Button variant="secondary" className="px-2 py-1 text-xs" onClick={() => { setSalary(r); setForm({ sum: '', comment: '', advance: false }) }}>
                          Оклад
                        </Button>
                        <Button variant="secondary" className="px-2 py-1 text-xs" onClick={() => { setExtra({ row: r, kind: 'bonus' }); setForm({ sum: '', comment: '', advance: false }) }}>
                          Бонус
                        </Button>
                        <Button variant="secondary" className="px-2 py-1 text-xs" onClick={() => { setExtra({ row: r, kind: 'penalty' }); setForm({ sum: '', comment: '', advance: false }) }}>
                          Удержать
                        </Button>
                      </>
                    )}
                    <Button
                      className="px-2 py-1 text-xs"
                      disabled={r.balance_tyiyn <= 0 && !owner}
                      onClick={() => {
                        setPaying(r)
                        setForm({ sum: somInput(Math.max(0, r.balance_tyiyn)), comment: '', advance: false })
                      }}
                    >
                      Выплатить
                    </Button>
                  </div>
                </td>
              </tr>
            ))}
          </Table>
        )}
      </Card>

      {paying && (
        <Modal title={`Выплата: ${paying.full_name}`} onClose={close}>
          <div className="flex flex-col gap-3">
            <div className="text-sm text-slate-600">К выплате {formatSom(paying.balance_tyiyn)}, деньги уйдут из кассы</div>
            <Field label="Сумма, с" required>
              <input autoFocus inputMode="decimal" value={form.sum} onChange={(e) => setForm({ ...form, sum: e.target.value })} />
            </Field>
            <Field label="Комментарий">
              <input value={form.comment} onChange={(e) => setForm({ ...form, comment: e.target.value })} />
            </Field>
            {overpay &&
              (owner ? (
                <label className="flex items-center gap-2 rounded-md bg-amber-50 px-3 py-2 text-sm text-amber-900">
                  <input type="checkbox" checked={form.advance} onChange={(e) => setForm({ ...form, advance: e.target.checked })} />
                  Это больше заработанного на {formatSom((parseSom(form.sum) ?? 0) - paying.balance_tyiyn)}. Выдать как аванс?
                </label>
              ) : (
                <div className="rounded-md bg-amber-50 px-3 py-2 text-sm text-amber-800">
                  Это больше заработанного: аванс выдаёт только владелец. Уменьшите сумму до {formatSom(Math.max(paying.balance_tyiyn, 0))}.
                </div>
              ))}
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={close}>
                Отмена
              </Button>
              <Button disabled={act.busy || !form.sum.trim() || (overpay && (!owner || !form.advance))} onClick={pay}>
                Выплатить
              </Button>
            </div>
          </div>
        </Modal>
      )}

      {extra && (
        <Modal title={`${extra.kind === 'bonus' ? 'Бонус' : 'Удержание'}: ${extra.row.full_name}`} onClose={close}>
          <div className="flex flex-col gap-3">
            <Field label="Сумма, с" required>
              <input autoFocus inputMode="decimal" value={form.sum} onChange={(e) => setForm({ ...form, sum: e.target.value })} />
            </Field>
            <Field label="Причина" required>
              <input value={form.comment} onChange={(e) => setForm({ ...form, comment: e.target.value })} />
            </Field>
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={close}>
                Отмена
              </Button>
              <Button disabled={act.busy || !form.sum.trim() || !form.comment.trim()} onClick={addExtra}>
                Записать
              </Button>
            </div>
          </div>
        </Modal>
      )}

      {salary && (
        <Modal title={`Оклад за месяц: ${salary.full_name}`} onClose={close}>
          <div className="flex flex-col gap-3">
            <div className="text-sm text-slate-600">
              Месяц {date.slice(0, 7)}. Пустая сумма — возьмём из правила оплаты.
            </div>
            <Field label="Сумма, с">
              <input autoFocus inputMode="decimal" value={form.sum} onChange={(e) => setForm({ ...form, sum: e.target.value })} />
            </Field>
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={close}>
                Отмена
              </Button>
              <Button disabled={act.busy} onClick={payMonth}>
                Начислить
              </Button>
            </div>
          </div>
        </Modal>
      )}
      </>
      )}
    </div>
  )
}
