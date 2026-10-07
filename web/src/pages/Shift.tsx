import { useState } from 'react'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, Missing, Modal, PageHeader, Table, toast } from '../components/ui'
import { get, newOpId, post } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatDateTime, formatSom, parseSom, somInput } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import type { CashAccount, Employee, Shift as ShiftRow } from '../lib/types'

const KIND_LABELS: Record<string, string> = {
  sale: 'Продажи',
  sale_return: 'Возвраты',
  cash_in: 'Внесения',
  cash_out: 'Изъятия',
  transfer_in: 'Переводы сюда',
  transfer_out: 'Переводы отсюда',
  bank_fee: 'Комиссия банка',
  expense: 'Расходы',
  payout: 'Выплаты',
  supplier_payment: 'Оплата поставщикам',
  debt_repayment: 'Погашения долгов',
  count_diff: 'Пересчёт',
  reversal: 'Сторно',
}

/** Смена и кассы: открыть, внести, изъять, перевести, закрыть с пересчётом (SPEC-05). */
export default function Shift() {
  const owner = useUser().role === 'owner'
  const current = useLoad(() => get<ShiftRow | null>('/shifts/current'), [])
  const accounts = useLoad(() => get<CashAccount[]>('/cash/accounts'), [])
  const employees = useLoad(() => get<Employee[]>('/employees'), [])
  const past = useLoad(() => get<ShiftRow[]>('/shifts'), [])
  const [open, setOpen] = useState<{ cashier: string; counted: string } | null>(null)
  const [money, setMoney] = useState<null | 'cash_in' | 'cash_out' | 'transfer'>(null)
  const [closing, setClosing] = useState<{ counted: string; comment: string } | null>(null)
  const [form, setForm] = useState({ sum: '', comment: '', from: '', to: '' })
  const act = useAction()

  const shift = current.data ?? null
  const cashiers = (employees.data ?? []).filter((e) => e.active && e.is_cashier)
  const accs = accounts.data ?? []
  const reloadAll = () => {
    current.reload()
    accounts.reload()
    past.reload()
  }

  const openShift = () =>
    void act.run(async () => {
      if (!open) return
      const counted = open.counted.trim() ? parseSom(open.counted) : null
      if (open.counted.trim() && counted === null) throw new Error('Неверная сумма')
      await post('/shifts/open', {
        op_id: newOpId(),
        cashier_employee_id: open.cashier,
        counted_tyiyn: counted,
      })
      setOpen(null)
      reloadAll()
      toast('Смена открыта')
    })

  const closeShift = () =>
    void act.run(async () => {
      if (!shift || !closing) return
      const counted = parseSom(closing.counted)
      if (counted === null) throw new Error('Неверная сумма')
      const res = await post<ShiftRow>(`/shifts/${shift.id}/close`, {
        op_id: newOpId(),
        counted_tyiyn: counted,
        comment: closing.comment,
      })
      setClosing(null)
      reloadAll()
      toast('Смена закрыта')
      // Деньги сдают сразу: подставляем всё, что в кассе, кассир оставит размен.
      const safe = accs.find((a) => a.kind === 'safe')
      if (safe && res.expected_tyiyn > 0) {
        setForm({ sum: somInput(res.expected_tyiyn), comment: 'сдача кассы', from: res.account_id, to: safe.id })
        setMoney('transfer')
      }
    })

  const submitMoney = () =>
    void act.run(async () => {
      const sum = parseSom(form.sum)
      if (sum === null || sum <= 0) throw new Error('Неверная сумма')
      if (money === 'transfer') {
        if (!form.from || !form.to) throw new Error('Выберите кассы')
        await post('/cash/transfers', {
          op_id: newOpId(),
          from_account_id: form.from,
          to_account_id: form.to,
          amount_tyiyn: sum,
          comment: form.comment,
        })
      } else {
        await post('/cash/movements', {
          op_id: newOpId(),
          kind: money,
          amount_tyiyn: sum,
          comment: form.comment,
        })
      }
      setMoney(null)
      setForm({ sum: '', comment: '', from: '', to: '' })
      reloadAll()
      toast('Записано')
    })

  const openMissing = missingWithFocus([Boolean(open?.cashier), 'кассира', '#shift-cashier'])

  return (
    <div className="flex flex-col gap-4">
      <PageHeader title="Смена и кассы" />
      <ErrorBox error={current.error ?? accounts.error ?? act.error} />

      {current.loading && !current.data ? (
        <Loading />
      ) : shift ? (
        <Card className="flex flex-col gap-3">
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div>
              <div className="text-xs text-slate-500">
                Смена № {shift.number} от {shift.business_date}, кассир {shift.cashier_name}
              </div>
              <div className="text-xs text-slate-500">Открыл {shift.opened_by}, {formatDateTime(shift.opened_at)}</div>
            </div>
            <div className="text-right">
              <div className="text-xs text-slate-500">Должно быть в кассе</div>
              <div className="text-3xl font-bold">{formatSom(shift.expected_tyiyn)}</div>
            </div>
          </div>
          <div className="flex flex-wrap gap-4 text-sm">
            <span>Наличными: {formatSom(shift.cash_sales_tyiyn)}</span>
            <span>Картой: {formatSom(shift.card_tyiyn)}</span>
            <span>Переводом: {formatSom(shift.transfer_tyiyn)}</span>
            <span>В долг: {formatSom(shift.debt_tyiyn)}</span>
          </div>
          {shift.breakdown.length > 0 && (
            <div className="flex flex-wrap gap-x-6 gap-y-1 border-t border-slate-200 pt-2 text-sm">
              {shift.breakdown.map((b) => (
                <span key={b.kind}>
                  {KIND_LABELS[b.kind] ?? b.kind}: {formatSom(b.sum_tyiyn)}
                </span>
              ))}
            </div>
          )}
          <div className="flex flex-wrap gap-2">
            <Button variant="secondary" onClick={() => setMoney('cash_in')}>
              Внести
            </Button>
            <Button variant="secondary" onClick={() => setMoney('cash_out')}>
              Изъять
            </Button>
            <Button
              variant="secondary"
              onClick={() => {
                setForm({ sum: '', comment: '', from: shift.account_id, to: accs.find((a) => a.kind === 'safe')?.id ?? '' })
                setMoney('transfer')
              }}
            >
              Перевести
            </Button>
            <Button onClick={() => setClosing({ counted: somInput(shift.expected_tyiyn), comment: '' })}>
              Закрыть смену
            </Button>
          </div>
        </Card>
      ) : (
        <Card className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <div className="font-medium">Смена не открыта</div>
            <div className="text-sm text-slate-500">Продавать можно и так, но деньги лучше считать сменами.</div>
          </div>
          <Button onClick={() => setOpen({ cashier: cashiers.length === 1 ? cashiers[0].id : '', counted: '' })}>
            Открыть смену
          </Button>
        </Card>
      )}

      <Card>
        <h2 className="mb-3 font-semibold">Кассы</h2>
        {accs.length === 0 ? (
          <Loading />
        ) : (
          <div className="flex flex-wrap gap-6">
            {accs.map((a) => (
              <div key={a.id}>
                <div className="text-xs text-slate-500">
                  {a.name}
                  {a.owner_only && ' · только владелец'}
                </div>
                <div className="text-xl font-bold">{formatSom(a.balance_tyiyn)}</div>
              </div>
            ))}
          </div>
        )}
      </Card>

      <Card>
        <h2 className="mb-3 font-semibold">Прошлые смены</h2>
        {(past.data ?? []).length === 0 ? (
          <Empty>Смен ещё не было</Empty>
        ) : (
          <Table head={['№', 'День', 'Кассир', 'Ожидали', 'Факт', 'Расхождение']}>
            {(past.data ?? []).map((s) => (
              <tr key={s.id}>
                <td className="px-2 py-2 font-medium">{s.number}</td>
                <td className="whitespace-nowrap px-2 py-2">{s.business_date}</td>
                <td className="px-2 py-2">{s.cashier_name}</td>
                <td className="whitespace-nowrap px-2 py-2">
                  {s.counted_tyiyn === null ? '—' : formatSom(s.counted_tyiyn - (s.diff_tyiyn ?? 0))}
                </td>
                <td className="whitespace-nowrap px-2 py-2">{s.counted_tyiyn === null ? 'открыта' : formatSom(s.counted_tyiyn)}</td>
                <td className="whitespace-nowrap px-2 py-2">
                  {s.diff_tyiyn ? <Badge tone={s.diff_tyiyn < 0 ? 'rose' : 'amber'}>{formatSom(s.diff_tyiyn)}</Badge> : '—'}
                </td>
              </tr>
            ))}
          </Table>
        )}
      </Card>

      {open && (
        <Modal title="Открыть смену" onClose={() => setOpen(null)}>
          <div className="flex flex-col gap-3">
            <Field label="Кассир" required>
              <select id="shift-cashier" value={open.cashier} onChange={(e) => setOpen({ ...open, cashier: e.target.value })}>
                <option value="">— выберите —</option>
                {cashiers.map((e) => (
                  <option key={e.id} value={e.id}>
                    {e.full_name}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Пересчитал, с" hint="Если не пересчитывали — оставьте пустым">
              <input inputMode="decimal" value={open.counted} onChange={(e) => setOpen({ ...open, counted: e.target.value })} />
            </Field>
            <Missing items={openMissing} />
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setOpen(null)}>
                Отмена
              </Button>
              <Button disabled={act.busy || openMissing.length > 0} onClick={openShift}>
                Открыть
              </Button>
            </div>
          </div>
        </Modal>
      )}

      {closing && shift && (
        <Modal title="Закрыть смену" onClose={() => setClosing(null)}>
          <div className="flex flex-col gap-3">
            <div className="text-sm text-slate-600">Должно быть в кассе: {formatSom(shift.expected_tyiyn)}</div>
            <Field label="Факт в кассе, с" required>
              <input autoFocus inputMode="decimal" value={closing.counted} onChange={(e) => setClosing({ ...closing, counted: e.target.value })} />
            </Field>
            {(() => {
              const counted = parseSom(closing.counted)
              const diff = counted === null ? null : counted - shift.expected_tyiyn
              if (diff === null || diff === 0) return null
              return (
                <div className={`rounded-md px-3 py-2 text-sm ${diff < 0 ? 'bg-rose-50 text-rose-800' : 'bg-amber-50 text-amber-800'}`}>
                  {diff < 0 ? 'Недостача' : 'Излишек'} {formatSom(Math.abs(diff))}
                  {diff < 0 && ` · будет удержана с кассира ${shift.cashier_name}`}
                </div>
              )
            })()}
            <Field label="Комментарий" hint="Обязателен при расхождении">
              <input value={closing.comment} onChange={(e) => setClosing({ ...closing, comment: e.target.value })} />
            </Field>
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setClosing(null)}>
                Отмена
              </Button>
              <Button disabled={act.busy || !closing.counted.trim()} onClick={closeShift}>
                Закрыть
              </Button>
            </div>
          </div>
        </Modal>
      )}

      {money && (
        <Modal
          title={money === 'cash_in' ? 'Внести в кассу' : money === 'cash_out' ? 'Изъять из кассы' : 'Перевести между кассами'}
          onClose={() => setMoney(null)}
        >
          <div className="flex flex-col gap-3">
            {money === 'transfer' && (
              <div className="grid grid-cols-2 gap-3">
                <Field label="Откуда" required>
                  <select value={form.from} onChange={(e) => setForm({ ...form, from: e.target.value })}>
                    <option value="">—</option>
                    {accs.map((a) => (
                      <option key={a.id} value={a.id}>
                        {a.name}
                      </option>
                    ))}
                  </select>
                </Field>
                <Field label="Куда" required>
                  <select value={form.to} onChange={(e) => setForm({ ...form, to: e.target.value })}>
                    <option value="">—</option>
                    {accs.map((a) => (
                      <option key={a.id} value={a.id}>
                        {a.name}
                      </option>
                    ))}
                  </select>
                </Field>
              </div>
            )}
            <Field label="Сумма, с" required>
              <input autoFocus inputMode="decimal" value={form.sum} onChange={(e) => setForm({ ...form, sum: e.target.value })} />
            </Field>
            <Field label="Комментарий" required={money !== 'transfer'}>
              <input value={form.comment} onChange={(e) => setForm({ ...form, comment: e.target.value })} />
            </Field>
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setMoney(null)}>
                Отмена
              </Button>
              <Button disabled={act.busy || !form.sum.trim()} onClick={submitMoney}>
                Записать
              </Button>
            </div>
          </div>
        </Modal>
      )}

      {!owner && <div className="text-xs text-slate-400">Сейф владельца и его остаток администратору не показываются.</div>}
    </div>
  )
}
