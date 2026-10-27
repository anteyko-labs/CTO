import { useState } from 'react'
import { Link } from 'react-router-dom'
import { Badge, Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, Missing, Modal, Money, PageHeader, Table, toast } from '../components/ui'
import { Icon } from '../components/icons'
import { Pager, usePageState, usePager } from '../components/Pager'
import { CorrectionBadges, CorrectionBanner, FixButton, useCorrection } from '../components/Correction'
import { get, getPage, newOpId, post, put } from '../lib/api'
import { useUser } from '../lib/auth'
import { useCan } from '../lib/permissions'
import { ShiftOpenModal } from '../components/ShiftOpen'
import { StaffPayList } from '../components/StaffPayList'
import { ShiftBank } from '../components/ShiftBank'
import { useShiftClose } from '../components/ShiftClose'
import { printShift } from '../components/shiftPrint'
import { formatDate, formatDateTime, formatSom, parseSom } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad, usePolling } from '../lib/hooks'
import { MOVEMENT_LABELS, shiftSales } from '../lib/income'
import { accountLabel, isTill, overdrawText } from '../lib/accounts'
import { correctionMarks, loadCorrections, numberText } from '../lib/corrections'
import { movementDoc, movementFields, transferDoc, transferFields, type MovementDoc, type TransferDoc } from '../lib/moneyFix'
import type { CashAccount, CashMovement, Employee, Shift as ShiftRow, ShiftSettings } from '../lib/types'

/** Виды движений словами — общие с «Счётом» и «Куда сколько поступило». */
const KIND_LABELS = MOVEMENT_LABELS

/** Движения наличных словами: «+ продажи наличными». */
const CASH_LABELS: Record<string, string> = {
  sale: 'Продажи наличными',
  sale_return: 'Возвраты наличными',
  cash_in: 'Внесли',
  cash_out: 'Изъяли',
  transfer_in: 'Переведено в кассу',
  transfer_out: 'Переведено из кассы',
  expense: 'Расходы из кассы',
  payout: 'Выплаты сотрудникам',
  supplier_payment: 'Оплата поставщикам',
  supplier_refund: 'Возмещения за брак',
  debt_repayment: 'Погашения долгов',
  battery_intake: 'Приём аккумуляторов',
  count_diff: 'Пересчёт',
  reversal: 'Сторно',
}

/** Строка отчёта: подпись и сумма, со знаком для движений. */
function Row({ label, value, signed = false, strong = false }: { label: string; value: number; signed?: boolean; strong?: boolean }) {
  return (
    <div className={`flex items-baseline justify-between gap-3 py-0.5 ${strong ? 'mt-1 border-t border-slate-200 pt-1 font-semibold' : 'text-slate-700'}`}>
      <span>{label}</span>
      <span className={`whitespace-nowrap tabular-nums ${signed && value < 0 ? 'text-rose-700' : ''}`}>
        {signed ? `${value < 0 ? '−' : '+'} ${formatSom(Math.abs(value))}` : formatSom(value)}
      </span>
    </div>
  )
}

/** Исправление проведённого внесения, изъятия или перевода (SPEC-29 §24): то же окно, заполненное
 *  исходным документом; `filled` — поля уже взяты из него. */
type MoneyFix = { kind: 'cash_movement' | 'cash_transfer'; id: string; filled: boolean }

type MoneyForm = { kind: 'cash_in' | 'cash_out' | 'transfer'; sum: string; comment: string; from: string; to: string; opId: string; fix?: MoneyFix }

/** Смена и кассы: открыть, внести, изъять, перевести, закрыть с пересчётом и сдать кассу (SPEC-05). */
export default function Shift() {
  const owner = useUser().role === 'owner'
  // Внести, изъять, перевести и выплатить — права администратора (ADR-097).
  const can = useCan()
  const current = useLoad(() => get<ShiftRow | null>('/shifts/current'), [])
  const accounts = useLoad(() => get<CashAccount[]>('/cash/accounts'), [])
  const employees = useLoad(() => get<Employee[]>('/employees'), [])
  const past = useLoad(() => get<ShiftRow[]>('/shifts'), [])
  // «Должно быть в кассе» и остатки касс живые: продажи с другого устройства видны сразу.
  usePolling(current.reload)
  usePolling(accounts.reload)
  // Окно «Открыть смену»: кассир, PIN или пароль владельца, пересчёт (ShiftOpenModal).
  const [open, setOpen] = useState(false)
  const [money, setMoney] = useState<MoneyForm | null>(null)
  const correction = useCorrection<MovementDoc | TransferDoc>(money?.fix?.kind ?? 'cash_movement', money?.fix?.id ?? null)
  const [reopen, setReopen] = useState<{ shift: ShiftRow; reason: string; opId: string } | null>(null)
  const [history, setHistory] = useState<string>('')
  const [reverse, setReverse] = useState<{ m: CashMovement; comment: string; opId: string } | null>(null)
  // Движения открытой кассы перечитываются по счётчику: после любой операции на экране.
  const [movesTick, setMovesTick] = useState(0)
  const act = useAction()

  const shift = current.data ?? null
  // Продано − возвраты = итого = сумма по способам оплаты: числа на экране складываются.
  const sales = shift ? shiftSales(shift) : null
  const cashiers = (employees.data ?? []).filter((e) => e.active && e.is_cashier)
  const accs = accounts.data ?? []
  // Выручку сдают в сейф магазина; в сейф владельца деньги перекладывает он сам (ADR-037).
  const shopSafe = accs.find((a) => a.kind === 'safe' && !a.owner_only)
  const pending = (past.data ?? []).find((s) => s.handover_pending)
  const reloadAll = () => {
    current.reload()
    accounts.reload()
    past.reload()
    setMovesTick((t) => t + 1)
  }
  const pastPager = usePager(past.data ?? [], 'shift.past')
  // Закрытие смены и «Сдать кассу» — то же окно, что в полосе режима кассы (SPEC-36 §10).
  const closer = useShiftClose(reloadAll)
  const closeDialogs = () => {
    act.setError(null)
    setOpen(false)
    setMoney(null)
    closer.cancel()
    setReopen(null)
    setReverse(null)
  }
  // Чек смены 80 мм (SPEC-36 §10): печать из списка смен и с открытой смены.
  const printCheck = (id: string) => void act.run(() => printShift(id))
  const settings = useLoad(() => get<ShiftSettings>('/settings/shift'), [])
  const togglePrint = (v: boolean) => {
    // Отметка меняется сразу; не сохранилось — возвращается как было.
    settings.setData({ print_on_close: v })
    void act.run(async () => {
      try {
        settings.setData(await put<ShiftSettings>('/settings/shift', { print_on_close: v }))
      } catch (e) {
        settings.setData({ print_on_close: !v })
        throw e
      }
      toast(v ? 'Чек смены будет печататься после сдачи кассы' : 'Чек смены сам не печатается')
    })
  }

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
          // Владелец вносит и изымает на любом счёте; без выбора — касса смены (ADR-076).
          account_id: money.from || undefined,
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

  const startFix = (m: CashMovement) => {
    closeDialogs()
    correction.setComment('')
    const transfer = m.doc_type === 'transfer'
    setMoney({
      kind: transfer ? 'transfer' : m.kind === 'cash_in' ? 'cash_in' : 'cash_out',
      sum: '',
      comment: '',
      from: '',
      to: '',
      opId: newOpId(),
      fix: { kind: transfer ? 'cash_transfer' : 'cash_movement', id: transfer ? (m.doc_id ?? '') : m.id, filled: false },
    })
  }

  const submitFix = async () => {
    if (!money?.fix) return
    const doc = money.fix.kind === 'cash_transfer' ? transferDoc(money) : movementDoc(money, till?.id ?? null)
    if (!doc) return
    const c = await correction.save(doc)
    if (!c) return
    setMoney(null)
    reloadAll()
    toast(`Исправлено: ${numberText(c.new_number) || (money.kind === 'transfer' ? 'перевод' : 'запись')}`)
  }

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
  const fromAccounts = owner ? accs : accs.filter((a) => isTill(a))
  // Касса смены: её остаток — «должно быть» на начало новой смены (SPEC-29 §16).
  const till = accs.find((a) => isTill(a))
  // Списание больше остатка — предупреждение, не запрет (ADR-077); при исправлении не показываем:
  // исходная сумма вернётся на счёт.
  const moneySource = money && money.kind !== 'cash_in' ? (money.from ? accs.find((a) => a.id === money.from) : till) : undefined
  const moneyOverdraw = money && !money.fix && moneySource ? overdrawText(moneySource.balance_tyiyn, parseSom(money.sum)) : ''
  // Исправление: поля окна заполняются исходным документом, как только он пришёл с сервера.
  const fixDraft = correction.draft.data
  if (money?.fix && !money.fix.filled && fixDraft && fixDraft.original_id === money.fix.id) {
    const fields =
      money.fix.kind === 'cash_transfer' ? transferFields(fixDraft.doc as TransferDoc) : movementFields(fixDraft.doc as MovementDoc, till?.id ?? null)
    setMoney({ ...money, ...fields, fix: { ...money.fix, filled: true } })
  }
  const moneyMissing = money
    ? missingWithFocus(
        [(parseSom(money.sum) ?? 0) > 0, 'сумму', '#money-sum'],
        [money.kind !== 'transfer' || Boolean(money.from), 'откуда', '#money-from'],
        [money.kind !== 'transfer' || Boolean(money.to), 'куда', '#money-to'],
        [money.kind !== 'transfer' || money.from !== money.to, 'разные кассы', '#money-to'],
        [money.kind === 'transfer' || Boolean(money.comment.trim()), 'за что', '#money-comment'],
        [!money.fix || Boolean(correction.comment.trim()), 'что и почему исправляете', '#correction-comment'],
      )
    : []

  return (
    <div className="flex flex-col gap-4">
      <PageHeader title="Смена и кассы" />
      <ErrorBox error={current.error ?? accounts.error} />

      {pending && !closer.open && (
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
              closer.startHandover(pending)
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
                Смена № {shift.number} от {formatDate(shift.business_date)}, кассир {shift.cashier_name}
              </div>
              <div className="text-xs text-slate-500">Открыл {shift.opened_by}, {formatDateTime(shift.opened_at)}</div>
            </div>
            <div className="text-right">
              <div className="text-xs text-slate-500">Должно быть в кассе</div>
              <div className="text-3xl font-bold tabular-nums">{formatSom(shift.expected_tyiyn)}</div>
            </div>
          </div>
          <div className="grid gap-4 text-sm lg:grid-cols-3">
            <div>
              <div className="mb-1 font-medium">Наличные в кассе</div>
              <Row label="На начало (размен)" value={shift.opening_expected_tyiyn} />
              {shift.breakdown
                .filter((b) => b.sum_tyiyn !== 0)
                .map((b) => (
                  <Row key={b.kind} label={CASH_LABELS[b.kind] ?? KIND_LABELS[b.kind] ?? b.kind} value={b.sum_tyiyn} signed />
                ))}
              <Row label="Должно быть в кассе" value={shift.expected_tyiyn} strong />
            </div>
            {sales && (
            <div data-testid="shift-sales">
              <div className="mb-1 font-medium">Продажи смены</div>
              <Row label={`Продано: ${sales.count} чек.`} value={sales.gross} />
              {sales.returns !== 0 && <Row label="Возвраты" value={-sales.returns} signed />}
              {sales.returns !== 0 && <Row label="Итого за вычетом возвратов" value={sales.net} strong />}
              {sales.methods.length > 0 && (
                <div className="mt-2 text-xs text-slate-500">По способам оплаты{sales.returns !== 0 ? ' (с учётом возвратов)' : ''}:</div>
              )}
              {sales.methods.map((m) => (
                <Row key={m.key} label={m.label} value={m.value} />
              ))}
              {sales.rest !== 0 && <Row label="Исправления и прочее" value={sales.rest} signed />}
              {/* Безнал по счетам — всегда, даже при одном счёте (SPEC-36 §9). */}
              <div className="mt-3">
                <ShiftBank list={shift.bank_split ?? []} />
              </div>
            </div>
            )}
            <div>
              <div className="mb-1 font-medium">Заработали в этот день</div>
              <StaffPayList staff={shift.pay ?? []} />
            </div>
          </div>
          <div className="flex flex-wrap gap-2">
            {can('cash_moves') && (
              <>
                <Button variant="secondary" onClick={() => startMoney('cash_in')}>
                  Внести
                </Button>
                <Button variant="secondary" onClick={() => startMoney('cash_out')}>
                  Изъять
                </Button>
                <Button variant="secondary" onClick={() => startMoney('transfer')}>
                  Перевести
                </Button>
              </>
            )}
            {/* Отчёт смены, как в Умаге (SPEC-30 §7): движение денег, выручка, операции, отменённые товары. */}
            <Link
              to={`/shifts/${shift.id}`}
              className="inline-flex min-h-[42px] items-center justify-center gap-2 rounded-md border border-slate-300 bg-white px-4 py-2 text-sm font-medium text-slate-800 hover:bg-slate-50"
            >
              <Icon name="history" className="h-4 w-4" />
              Отчёт смены
            </Link>
            <Button variant="secondary" onClick={() => printCheck(shift.id)}>
              Печать чека смены
            </Button>
            <Button
              onClick={() => {
                closeDialogs()
                closer.startClosing()
              }}
            >
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
          <div className="flex flex-wrap gap-2">
            {owner && (
              <Button variant="secondary" onClick={() => startMoney('transfer')}>
                Перевести
              </Button>
            )}
            <Button
              onClick={() => {
                closeDialogs()
                setOpen(true)
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
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
            {accs.map((a) => (
              <button
                key={a.id}
                type="button"
                aria-pressed={history === a.id}
                className={`flex items-center justify-between gap-3 rounded-md border px-3 py-2 text-left transition hover:border-sky-400 hover:bg-slate-50 ${
                  history === a.id ? 'border-sky-500 bg-sky-50' : 'border-slate-200'
                }`}
                onClick={() => setHistory(history === a.id ? '' : a.id)}
              >
                <div className="min-w-0">
                  <div className="text-xs text-slate-500">
                    {a.name}
                    {a.owner_only && ' · только владелец'}
                  </div>
                  <Money value={a.balance_tyiyn} tone="negative" className="text-xl font-bold" />
                </div>
                <Icon name="chevron" className={`h-4 w-4 shrink-0 text-slate-400 transition ${history === a.id ? 'rotate-90 text-sky-600' : ''}`} />
              </button>
            ))}
          </div>
        )}
        {!history && accs.length > 0 && <div className="mt-2 text-xs text-slate-400">Нажмите на кассу, чтобы увидеть её движения.</div>}
        {history && (
          <AccountMoves
            id={history}
            tick={movesTick}
            onReverse={(m) => {
              closeDialogs()
              setReverse({ m, comment: '', opId: newOpId() })
            }}
            onFix={startFix}
          />
        )}
      </Card>

      <Card>
        <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
          <h2 className="font-semibold">Прошлые смены</h2>
          {owner && settings.data && (
            <div data-testid="print-on-close">
              <Checkbox label="Печатать чек смены после сдачи кассы" checked={settings.data.print_on_close} onChange={togglePrint} />
            </div>
          )}
        </div>
        <ErrorBox error={past.error ?? (closer.open || money || reopen || reverse ? null : act.error)} />
        {(past.data ?? []).length === 0 ? (
          <Empty>Смен ещё не было</Empty>
        ) : (
          <Table head={['№', 'День', 'Кассир', 'Ожидали', 'Факт', 'Расхождение', 'Сдано', '']}>
            {pastPager.shown.map((s, i) => (
              <tr key={s.id}>
                <td className="px-2 py-2 font-medium">
                  <Link to={`/shifts/${s.id}`} className="text-sky-700 hover:underline" aria-label={`Отчёт смены № ${s.number}`}>
                    {s.number}
                  </Link>
                </td>
                <td className="whitespace-nowrap px-2 py-2 tabular-nums">{formatDate(s.business_date)}</td>
                <td className="px-2 py-2">{s.cashier_name}</td>
                <td className="px-2 py-2 text-right">{s.counted_tyiyn === null ? '—' : <Money value={s.expected_tyiyn} />}</td>
                <td className="px-2 py-2 text-right">{s.counted_tyiyn === null ? <Badge tone="green">открыта</Badge> : <Money value={s.counted_tyiyn} />}</td>
                <td className="whitespace-nowrap px-2 py-2 text-right">
                  {s.diff_tyiyn ? <Badge tone={s.diff_tyiyn < 0 ? 'rose' : 'amber'}>{formatSom(s.diff_tyiyn)}</Badge> : '—'}
                </td>
                <td className="whitespace-nowrap px-2 py-2 text-right">
                  {s.handover_pending ? <Badge tone="amber">не сдана</Badge> : s.to_safe_tyiyn === null ? '—' : <Money value={s.to_safe_tyiyn} />}
                </td>
                <td className="px-2 py-2 text-right">
                  <div className="flex flex-wrap items-center justify-end gap-2">
                  <Link to={`/shifts/${s.id}`} className="inline-flex min-h-9 items-center rounded-md border border-slate-300 bg-white px-3 py-1 text-sm font-medium text-slate-800 hover:bg-slate-50">
                    Отчёт
                  </Link>
                  <Button variant="secondary" className="min-h-9 px-3 py-1" aria-label={`Печать чека смены № ${s.number}`} onClick={() => printCheck(s.id)}>
                    Печать чека смены
                  </Button>
                  {/* Переоткрыть можно только последнюю смену: после более поздней — 409 (ADR-071). */}
                  {owner && pastPager.page === 0 && i === 0 && s.counted_tyiyn !== null && !shift && (
                    <Button
                      variant="secondary"
                      className="min-h-9 px-3 py-1"
                      onClick={() => {
                        closeDialogs()
                        setReopen({ shift: s, reason: '', opId: newOpId() })
                      }}
                    >
                      Переоткрыть
                    </Button>
                  )}
                  </div>
                </td>
              </tr>
            ))}
          </Table>
        )}
        <Pager page={pastPager.page} size={pastPager.size} total={pastPager.total} onPage={pastPager.setPage} onSize={pastPager.setSize} label="Страницы смен" />
      </Card>

      {open && (
        <ShiftOpenModal
          owner={owner}
          cashiers={cashiers}
          till={till}
          onClose={closeDialogs}
          onOpened={() => {
            setOpen(false)
            reloadAll()
          }}
        />
      )}

      {closer.modals}

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
          title={`${money.fix ? 'Исправить: ' : ''}${money.kind === 'cash_in' ? 'Внести в кассу' : money.kind === 'cash_out' ? 'Изъять из кассы' : 'Перевести между кассами'}`}
          onClose={closeDialogs}
        >
          <div className="flex flex-col gap-3">
            {money.fix && !money.fix.filled && (correction.draft.loading ? <Loading /> : null)}
            {money.fix && fixDraft && (
              <CorrectionBanner label={fixDraft.kind_label} number={fixDraft.number} comment={correction.comment} onComment={correction.setComment} />
            )}
            {money.kind === 'transfer' && (
              <div className="grid grid-cols-2 gap-3">
                <Field label="Откуда" required>
                  <select id="money-from" value={money.from} onChange={(e) => setMoney({ ...money, from: e.target.value })}>
                    <option value="">—</option>
                    {fromAccounts.map((a) => (
                      <option key={a.id} value={a.id}>
                        {accountLabel(a)}
                      </option>
                    ))}
                  </select>
                </Field>
                <Field label="Куда" required>
                  <select id="money-to" value={money.to} onChange={(e) => setMoney({ ...money, to: e.target.value })}>
                    <option value="">—</option>
                    {accs.map((a) => (
                      <option key={a.id} value={a.id}>
                        {accountLabel(a)}
                      </option>
                    ))}
                  </select>
                </Field>
              </div>
            )}
            {money.kind !== 'transfer' && owner && accs.length > 1 && (
              <Field label="Касса или счёт">
                <select id="money-account" value={money.from} onChange={(e) => setMoney({ ...money, from: e.target.value })}>
                  <option value="">Касса смены</option>
                  {accs
                    .filter((a) => !isTill(a))
                    .map((a) => (
                      <option key={a.id} value={a.id}>
                        {accountLabel(a)}
                      </option>
                    ))}
                </select>
              </Field>
            )}
            <Field label="Сумма, с" required>
              <input id="money-sum" autoFocus inputMode="decimal" value={money.sum} onChange={(e) => setMoney({ ...money, sum: e.target.value })} />
            </Field>
            <Field label="Комментарий" required={money.kind !== 'transfer'}>
              <input
                id="money-comment"
                value={money.comment}
                onChange={(e) => setMoney({ ...money, comment: e.target.value })}
                onKeyDown={(e) => {
                  // Enter в последнем поле — как главная кнопка (ui-rules §4).
                  if (e.key !== 'Enter' || moneyMissing.length > 0) return
                  e.preventDefault()
                  if (money.fix) {
                    if (!correction.busy && money.fix.filled) void submitFix()
                  } else if (!act.busy) submitMoney()
                }}
              />
            </Field>
            {moneyOverdraw && (
              <div className="rounded-md bg-amber-50 px-3 py-2 text-sm text-amber-800" data-testid="money-overdraw">
                {moneyOverdraw}
              </div>
            )}
            <Missing items={moneyMissing} />
            <ErrorBox error={money.fix ? correction.error : act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={closeDialogs}>
                Отмена
              </Button>
              {money.fix ? (
                <Button disabled={correction.busy || !money.fix.filled || moneyMissing.length > 0} onClick={() => void submitFix()}>
                  Сохранить исправление
                </Button>
              ) : (
                <Button disabled={act.busy || moneyMissing.length > 0} onClick={submitMoney}>
                  Записать
                </Button>
              )}
            </div>
          </div>
        </Modal>
      )}

      {!owner && <div className="text-xs text-slate-400">Сейф владельца и его остаток администратору не показываются.</div>}
    </div>
  )
}

/** Движения выбранной кассы, новые сверху; страницу режет сервер. `tick` — перечитать после операции. */
function AccountMoves({ id, tick, onReverse, onFix }: { id: string; tick: number; onReverse: (m: CashMovement) => void; onFix: (m: CashMovement) => void }) {
  const pager = usePageState('shift.movements', id)
  const moves = useLoad(() => getPage<CashMovement>(`/cash/accounts/${id}`, { limit: pager.size, offset: pager.page * pager.size }), [id, pager.page, pager.size, tick])
  // Отметки исправлений: внесения и изъятия — по id движения, переводы — по id перевода.
  const fixes = useLoad(() => Promise.all([loadCorrections('cash_movement'), loadCorrections('cash_transfer')]).then(([a, b]) => correctionMarks([...a, ...b])), [tick])
  const markOf = (m: CashMovement) => fixes.data?.get(m.doc_type === 'transfer' && m.doc_id ? m.doc_id : m.id)
  const rows = moves.data?.items ?? []
  const mayReverse = useCan()('cash_moves')
  return (
    <div className="mt-4">
      <ErrorBox error={moves.error} />
      {moves.loading && !moves.data ? (
        <Loading />
      ) : (moves.data?.total ?? 0) === 0 ? (
        <Empty>Движений ещё не было</Empty>
      ) : (
        <>
          <Table head={['Когда', 'Что', 'Сумма', 'Комментарий', 'Кто', '']}>
            {rows.map((m) => (
              <tr key={m.id}>
                <td className="whitespace-nowrap px-2 py-2">{formatDateTime(m.created_at)}</td>
                <td className="px-2 py-2">
                  {KIND_LABELS[m.kind] ?? m.kind}
                  <span className="ml-2 inline-flex flex-wrap gap-1">
                    <CorrectionBadges mark={markOf(m)} />
                  </span>
                </td>
                <td className="px-2 py-2 text-right">
                  <Money value={m.amount_tyiyn} tone="negative" className="font-medium" />
                </td>
                <td className="px-2 py-2 text-slate-600">{m.comment}</td>
                <td className="px-2 py-2">{m.user_name}</td>
                <td className="px-2 py-2 text-right">
                  {m.reversible && mayReverse && (
                    <div className="flex items-center justify-end gap-2">
                      <FixButton small onClick={() => onFix(m)} />
                      <Button variant="ghost" onClick={() => onReverse(m)}>
                        Сторно
                      </Button>
                    </div>
                  )}
                </td>
              </tr>
            ))}
          </Table>
          <Pager page={pager.page} size={pager.size} total={moves.data?.total ?? 0} onPage={pager.setPage} onSize={pager.setSize} label="Страницы движений кассы" />
        </>
      )}
    </div>
  )
}
