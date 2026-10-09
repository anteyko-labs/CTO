import { useState } from 'react'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, Missing, Modal, PageHeader, Table, toast } from '../components/ui'
import { get, newOpId, post } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatDateTime, formatSom, parseSom, somInput } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad, usePolling } from '../lib/hooks'
import type { CashAccount, CashMovement, Employee, Shift as ShiftRow } from '../lib/types'

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

type MoneyForm = { kind: 'cash_in' | 'cash_out' | 'transfer'; sum: string; comment: string; from: string; to: string; opId: string }

/** Смена и кассы: открыть, внести, изъять, перевести, закрыть с пересчётом и сдать кассу (SPEC-05). */
export default function Shift() {
  const owner = useUser().role === 'owner'
  const current = useLoad(() => get<ShiftRow | null>('/shifts/current'), [])
  const accounts = useLoad(() => get<CashAccount[]>('/cash/accounts'), [])
  const employees = useLoad(() => get<Employee[]>('/employees'), [])
  const past = useLoad(() => get<ShiftRow[]>('/shifts'), [])
  // «Должно быть в кассе» и остатки касс живые: продажи с другого устройства видны сразу.
  usePolling(current.reload)
  usePolling(accounts.reload)
  const [open, setOpen] = useState<{ cashier: string; counted: string; opId: string } | null>(null)
  const [money, setMoney] = useState<MoneyForm | null>(null)
  const [closing, setClosing] = useState<{ counted: string; comment: string; opId: string } | null>(null)
  const [handover, setHandover] = useState<{ shift: ShiftRow; sum: string; opId: string } | null>(null)
  const [reopen, setReopen] = useState<{ shift: ShiftRow; reason: string; opId: string } | null>(null)
  const [history, setHistory] = useState<string>('')
  const [reverse, setReverse] = useState<{ m: CashMovement; comment: string; opId: string } | null>(null)
  const moves = useLoad(() => (history ? get<CashMovement[]>(`/cash/accounts/${history}`) : Promise.resolve([])), [history])
  const act = useAction()

  const shift = current.data ?? null
  const cashiers = (employees.data ?? []).filter((e) => e.active && e.is_cashier)
  const accs = accounts.data ?? []
  // Выручку сдают в сейф магазина; в сейф владельца деньги перекладывает он сам (ADR-037).
  const shopSafe = accs.find((a) => a.kind === 'safe' && !a.owner_only)
  const pending = (past.data ?? []).find((s) => s.handover_pending)
  const reloadAll = () => {
    current.reload()
    accounts.reload()
    past.reload()
    moves.reload()
  }
  const closeDialogs = () => {
    act.setError(null)
    setOpen(null)
    setMoney(null)
    setClosing(null)
    setHandover(null)
    setReopen(null)
    setReverse(null)
  }

  const openShift = () =>
    void act.run(async () => {
      if (!open) return
      const counted = open.counted.trim() ? parseSom(open.counted) : null
      if (open.counted.trim() && counted === null) throw new Error('Неверная сумма')
      await post('/shifts/open', {
        op_id: open.opId,
        cashier_employee_id: open.cashier,
        counted_tyiyn: counted,
      })
      setOpen(null)
      reloadAll()
      toast('Смена открыта')
    })

  const startClosing = () => {
    closeDialogs()
    if (!shift) return
    // Сумму «должно быть» берём свежую: за время на экране могли пройти чеки с другого устройства.
    current.reload()
    setClosing({ counted: somInput(shift.expected_tyiyn), comment: '', opId: newOpId() })
  }

  const closeShift = () =>
    void act.run(async () => {
      if (!shift || !closing) return
      const counted = parseSom(closing.counted)
      if (counted === null) throw new Error('Неверная сумма')
      const res = await post<ShiftRow>(`/shifts/${shift.id}/close`, {
        op_id: closing.opId,
        counted_tyiyn: counted,
        comment: closing.comment,
      })
      setClosing(null)
      reloadAll()
      toast('Смена закрыта')
      // Деньги сдают сразу: подставляем всё, что пересчитали, кассир оставит размен.
      setHandover({ shift: res, sum: somInput(res.counted_tyiyn ?? 0), opId: newOpId() })
    })

  const submitHandover = (amount: number) =>
    void act.run(async () => {
      if (!handover) return
      await post(`/shifts/${handover.shift.id}/handover`, { op_id: handover.opId, amount_tyiyn: amount })
      setHandover(null)
      reloadAll()
      toast(amount > 0 ? 'Выручка в сейфе' : 'Отмечено: деньги остались в кассе')
    })

  const submitReopen = () =>
    void act.run(async () => {
      if (!reopen) return
      await post(`/shifts/${reopen.shift.id}/reopen`, { op_id: reopen.opId, reason: reopen.reason })
      setReopen(null)
      reloadAll()
      toast('Смена переоткрыта')
    })

  const submitMoney = () =>
    void act.run(async () => {
      if (!money) return
      const sum = parseSom(money.sum)
      if (sum === null || sum <= 0) throw new Error('Неверная сумма')
      if (money.kind === 'transfer') {
        await post('/cash/transfers', {
          op_id: money.opId,
          from_account_id: money.from,
          to_account_id: money.to,
          amount_tyiyn: sum,
          comment: money.comment,
        })
      } else {
        await post('/cash/movements', {
          op_id: money.opId,
          kind: money.kind,
          amount_tyiyn: sum,
          comment: money.comment,
        })
      }
      setMoney(null)
      reloadAll()
      toast('Записано')
    })

  const submitReverse = () =>
    void act.run(async () => {
      if (!reverse) return
      const { m } = reverse
      const path = m.doc_type === 'transfer' ? `/cash/transfers/${m.doc_id}/reverse` : `/cash/movements/${m.id}/reverse`
      await post(path, { op_id: reverse.opId, comment: reverse.comment })
      setReverse(null)
      reloadAll()
      toast('Сторно проведено')
    })

  const startMoney = (kind: MoneyForm['kind']) => {
    closeDialogs()
    setMoney({
      kind,
      sum: '',
      comment: '',
      from: kind === 'transfer' ? (shift?.account_id ?? '') : '',
      to: kind === 'transfer' ? (shopSafe?.id ?? '') : '',
      opId: newOpId(),
    })
  }

  // Администратор переводит только из кассы смены (SPEC-05, права).
  const fromAccounts = owner ? accs : accs.filter((a) => a.kind === 'register')
  const openMissing = missingWithFocus([Boolean(open?.cashier), 'кассира', '#shift-cashier'])
  const closeDiff = (() => {
    if (!closing || !shift) return null
    const counted = parseSom(closing.counted)
    return counted === null ? null : counted - shift.expected_tyiyn
  })()
  const closeMissing = missingWithFocus(
    [parseSom(closing?.counted ?? '') !== null, 'сколько денег в кассе', '#close-counted'],
    [!closeDiff || Boolean(closing?.comment.trim()), 'комментарий к расхождению', '#close-comment'],
  )
  const moneyMissing = money
    ? missingWithFocus(
        [(parseSom(money.sum) ?? 0) > 0, 'сумму', '#money-sum'],
        [money.kind !== 'transfer' || Boolean(money.from), 'откуда', '#money-from'],
        [money.kind !== 'transfer' || Boolean(money.to), 'куда', '#money-to'],
        [money.kind !== 'transfer' || money.from !== money.to, 'разные кассы', '#money-to'],
        [money.kind === 'transfer' || Boolean(money.comment.trim()), 'за что', '#money-comment'],
      )
    : []
  const handoverSum = handover ? parseSom(handover.sum) : null
  const handoverMax = handover ? (handover.shift.counted_tyiyn ?? 0) : 0

  return (
    <div className="flex flex-col gap-4">
      <PageHeader title="Смена и кассы" />
      <ErrorBox error={current.error ?? accounts.error} />

      {pending && !handover && (
        <Card className="flex flex-wrap items-center justify-between gap-3 border-amber-300 bg-amber-50">
          <div>
            <div className="font-medium">Смена № {pending.number} закрыта, касса не сдана</div>
            <div className="text-sm text-slate-600">
              Пересчитали {formatSom(pending.counted_tyiyn ?? 0)}. Переведите выручку в сейф или отметьте, что деньги остались в кассе.
            </div>
          </div>
          <Button
            onClick={() => {
              closeDialogs()
              setHandover({ shift: pending, sum: somInput(pending.counted_tyiyn ?? 0), opId: newOpId() })
            }}
          >
            Сдать кассу
          </Button>
        </Card>
      )}

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
            <span>QR: {formatSom(shift.transfer_tyiyn)}</span>
            {shift.bank_fee_tyiyn > 0 && <span>Комиссия банка: {formatSom(shift.bank_fee_tyiyn)}</span>}
            <span>В долг: {formatSom(shift.debt_tyiyn)}</span>
            {(shift.bonus_tyiyn ?? 0) !== 0 && <span>Баллами: {formatSom(shift.bonus_tyiyn ?? 0)}</span>}
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
            <Button variant="secondary" onClick={() => startMoney('cash_in')}>
              Внести
            </Button>
            <Button variant="secondary" onClick={() => startMoney('cash_out')}>
              Изъять
            </Button>
            <Button variant="secondary" onClick={() => startMoney('transfer')}>
              Перевести
            </Button>
            <Button onClick={startClosing}>Закрыть смену</Button>
          </div>
        </Card>
      ) : (
        <Card className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <div className="font-medium">Смена не открыта</div>
            <div className="text-sm text-slate-500">Продавать можно и так, но деньги лучше считать сменами.</div>
          </div>
          <div className="flex flex-wrap gap-2">
            {owner && (
              <Button variant="secondary" onClick={() => startMoney('transfer')}>
                Перевести
              </Button>
            )}
            <Button
              onClick={() => {
                closeDialogs()
                setOpen({ cashier: cashiers.length === 1 ? cashiers[0].id : '', counted: '', opId: newOpId() })
              }}
            >
              Открыть смену
            </Button>
          </div>
        </Card>
      )}

      <Card>
        <h2 className="mb-3 font-semibold">Кассы</h2>
        {accs.length === 0 ? (
          <Loading />
        ) : (
          <div className="flex flex-wrap gap-6">
            {accs.map((a) => (
              <button
                key={a.id}
                type="button"
                className={`rounded-md px-2 py-1 text-left hover:bg-slate-100 ${history === a.id ? 'bg-slate-100 ring-1 ring-slate-300' : ''}`}
                onClick={() => setHistory(history === a.id ? '' : a.id)}
              >
                <div className="text-xs text-slate-500">
                  {a.name}
                  {a.owner_only && ' · только владелец'}
                </div>
                <div className="text-xl font-bold">{formatSom(a.balance_tyiyn)}</div>
              </button>
            ))}
          </div>
        )}
        {!history && accs.length > 0 && <div className="mt-2 text-xs text-slate-400">Нажмите на кассу, чтобы увидеть её движения.</div>}
        {history && (
          <div className="mt-4">
            <ErrorBox error={moves.error} />
            {moves.loading && !moves.data ? (
              <Loading />
            ) : (moves.data ?? []).length === 0 ? (
              <Empty>Движений ещё не было</Empty>
            ) : (
              <Table head={['Когда', 'Что', 'Сумма', 'Комментарий', 'Кто', '']}>
                {(moves.data ?? []).map((m) => (
                  <tr key={m.id}>
                    <td className="whitespace-nowrap px-2 py-2">{formatDateTime(m.created_at)}</td>
                    <td className="px-2 py-2">{KIND_LABELS[m.kind] ?? m.kind}</td>
                    <td className={`whitespace-nowrap px-2 py-2 font-medium ${m.amount_tyiyn < 0 ? 'text-rose-700' : ''}`}>
                      {formatSom(m.amount_tyiyn)}
                    </td>
                    <td className="px-2 py-2 text-slate-600">{m.comment}</td>
                    <td className="px-2 py-2">{m.user_name}</td>
                    <td className="px-2 py-2 text-right">
                      {m.reversible && (
                        <Button
                          variant="ghost"
                          onClick={() => {
                            closeDialogs()
                            setReverse({ m, comment: '', opId: newOpId() })
                          }}
                        >
                          Сторно
                        </Button>
                      )}
                    </td>
                  </tr>
                ))}
              </Table>
            )}
          </div>
        )}
      </Card>

      <Card>
        <h2 className="mb-3 font-semibold">Прошлые смены</h2>
        <ErrorBox error={past.error} />
        {(past.data ?? []).length === 0 ? (
          <Empty>Смен ещё не было</Empty>
        ) : (
          <Table head={['№', 'День', 'Кассир', 'Ожидали', 'Факт', 'Расхождение', 'В сейф', '']}>
            {(past.data ?? []).map((s) => (
              <tr key={s.id}>
                <td className="px-2 py-2 font-medium">{s.number}</td>
                <td className="whitespace-nowrap px-2 py-2">{s.business_date}</td>
                <td className="px-2 py-2">{s.cashier_name}</td>
                <td className="whitespace-nowrap px-2 py-2">{s.counted_tyiyn === null ? '—' : formatSom(s.expected_tyiyn)}</td>
                <td className="whitespace-nowrap px-2 py-2">{s.counted_tyiyn === null ? 'открыта' : formatSom(s.counted_tyiyn)}</td>
                <td className="whitespace-nowrap px-2 py-2">
                  {s.diff_tyiyn ? <Badge tone={s.diff_tyiyn < 0 ? 'rose' : 'amber'}>{formatSom(s.diff_tyiyn)}</Badge> : '—'}
                </td>
                <td className="whitespace-nowrap px-2 py-2">
                  {s.handover_pending ? <Badge tone="amber">не сдана</Badge> : s.to_safe_tyiyn === null ? '—' : formatSom(s.to_safe_tyiyn)}
                </td>
                <td className="px-2 py-2 text-right">
                  {owner && s.counted_tyiyn !== null && !shift && (
                    <Button
                      variant="ghost"
                      onClick={() => {
                        closeDialogs()
                        setReopen({ shift: s, reason: '', opId: newOpId() })
                      }}
                    >
                      Переоткрыть
                    </Button>
                  )}
                </td>
              </tr>
            ))}
          </Table>
        )}
      </Card>

      {open && (
        <Modal title="Открыть смену" onClose={closeDialogs}>
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
              <Button variant="secondary" onClick={closeDialogs}>
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
        <Modal title="Закрыть смену" onClose={closeDialogs}>
          <div className="flex flex-col gap-3">
            <div className="text-sm text-slate-600">Должно быть в кассе: {formatSom(shift.expected_tyiyn)}</div>
            <Field label="Факт в кассе, с" required>
              <input
                id="close-counted"
                autoFocus
                inputMode="decimal"
                value={closing.counted}
                onChange={(e) => setClosing({ ...closing, counted: e.target.value })}
              />
            </Field>
            {closeDiff !== null && closeDiff !== 0 && (
              <div className={`rounded-md px-3 py-2 text-sm ${closeDiff < 0 ? 'bg-rose-50 text-rose-800' : 'bg-amber-50 text-amber-800'}`}>
                {closeDiff < 0 ? 'Недостача' : 'Излишек'} {formatSom(Math.abs(closeDiff))}
                {closeDiff < 0 && ` · будет удержана с кассира ${shift.cashier_name}`}
              </div>
            )}
            <Field label="Комментарий" required={Boolean(closeDiff)} hint="Обязателен при расхождении">
              <input id="close-comment" value={closing.comment} onChange={(e) => setClosing({ ...closing, comment: e.target.value })} />
            </Field>
            <Missing items={closeMissing} />
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={closeDialogs}>
                Отмена
              </Button>
              <Button disabled={act.busy || closeMissing.length > 0} onClick={closeShift}>
                Закрыть
              </Button>
            </div>
          </div>
        </Modal>
      )}

      {handover && (
        <Modal title={`Сдать кассу · смена № ${handover.shift.number}`} onClose={closeDialogs}>
          <div className="flex flex-col gap-3">
            <div className="text-sm text-slate-600">
              В кассе {formatSom(handoverMax)}. Сколько переложить в {shopSafe?.name ?? 'сейф магазина'}? Остальное останется на размен.
            </div>
            <Field label="В сейф, с" required>
              <input autoFocus inputMode="decimal" value={handover.sum} onChange={(e) => setHandover({ ...handover, sum: e.target.value })} />
            </Field>
            {handoverSum !== null && handoverSum >= 0 && handoverSum <= handoverMax && (
              <div className="text-sm text-slate-600">Останется в кассе: {formatSom(handoverMax - handoverSum)}</div>
            )}
            {handoverSum !== null && handoverSum > handoverMax && (
              <div className="text-sm text-rose-700">В кассе столько нет</div>
            )}
            <ErrorBox error={act.error} />
            <div className="flex flex-wrap justify-end gap-2">
              <Button variant="secondary" disabled={act.busy} onClick={() => submitHandover(0)}>
                Деньги остались в кассе
              </Button>
              <Button
                disabled={act.busy || handoverSum === null || handoverSum <= 0 || handoverSum > handoverMax}
                onClick={() => handoverSum !== null && submitHandover(handoverSum)}
              >
                Перевести в сейф
              </Button>
            </div>
            <div className="text-xs text-slate-400">Владелец получит уведомление: сколько было, сколько пересчитали, что ушло в сейф.</div>
          </div>
        </Modal>
      )}

      {reopen && (
        <Modal title={`Переоткрыть смену № ${reopen.shift.number}`} onClose={closeDialogs}>
          <div className="flex flex-col gap-3">
            <div className="text-sm text-slate-600">
              Пересчёт и удержание прошлого закрытия останутся в истории; при новом закрытии пересчёт пойдёт от текущего остатка.
            </div>
            <Field label="Причина" required>
              <input id="reopen-reason" autoFocus value={reopen.reason} onChange={(e) => setReopen({ ...reopen, reason: e.target.value })} />
            </Field>
            <Missing items={missingWithFocus([Boolean(reopen.reason.trim()), 'причину', '#reopen-reason'])} />
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={closeDialogs}>
                Отмена
              </Button>
              <Button disabled={act.busy || !reopen.reason.trim()} onClick={submitReopen}>
                Переоткрыть
              </Button>
            </div>
          </div>
        </Modal>
      )}

      {reverse && (
        <Modal title="Сторно" onClose={closeDialogs}>
          <div className="flex flex-col gap-3">
            <div className="text-sm text-slate-600">
              {KIND_LABELS[reverse.m.kind] ?? reverse.m.kind} на {formatSom(Math.abs(reverse.m.amount_tyiyn))}
              {reverse.m.comment && ` — ${reverse.m.comment}`}. Запись останется в истории, рядом появится обратная.
            </div>
            <Field label="Причина" required>
              <input id="reverse-comment" autoFocus value={reverse.comment} onChange={(e) => setReverse({ ...reverse, comment: e.target.value })} />
            </Field>
            <Missing items={missingWithFocus([Boolean(reverse.comment.trim()), 'причину', '#reverse-comment'])} />
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={closeDialogs}>
                Отмена
              </Button>
              <Button variant="danger" disabled={act.busy || !reverse.comment.trim()} onClick={submitReverse}>
                Провести сторно
              </Button>
            </div>
          </div>
        </Modal>
      )}

      {money && (
        <Modal
          title={money.kind === 'cash_in' ? 'Внести в кассу' : money.kind === 'cash_out' ? 'Изъять из кассы' : 'Перевести между кассами'}
          onClose={closeDialogs}
        >
          <div className="flex flex-col gap-3">
            {money.kind === 'transfer' && (
              <div className="grid grid-cols-2 gap-3">
                <Field label="Откуда" required>
                  <select id="money-from" value={money.from} onChange={(e) => setMoney({ ...money, from: e.target.value })}>
                    <option value="">—</option>
                    {fromAccounts.map((a) => (
                      <option key={a.id} value={a.id}>
                        {a.name}
                      </option>
                    ))}
                  </select>
                </Field>
                <Field label="Куда" required>
                  <select id="money-to" value={money.to} onChange={(e) => setMoney({ ...money, to: e.target.value })}>
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
              <input id="money-sum" autoFocus inputMode="decimal" value={money.sum} onChange={(e) => setMoney({ ...money, sum: e.target.value })} />
            </Field>
            <Field label="Комментарий" required={money.kind !== 'transfer'}>
              <input id="money-comment" value={money.comment} onChange={(e) => setMoney({ ...money, comment: e.target.value })} />
            </Field>
            <Missing items={moneyMissing} />
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={closeDialogs}>
                Отмена
              </Button>
              <Button disabled={act.busy || moneyMissing.length > 0} onClick={submitMoney}>
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
