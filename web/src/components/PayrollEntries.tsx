// Начисления и выплаты сотрудника за период с отменой (сторно) владельцем (SPEC-07, ADR-064)
// и исправлением той же формой, которой их создают (SPEC-29 §24, ADR-085).
import { useState } from 'react'
import { get, newOpId, post, qs } from '../lib/api'
import { correctionMarks, loadCorrections } from '../lib/corrections'
import { formatDate, formatDateTime, formatSom } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { CorrectionBadges, FixButton } from './Correction'
import { FixDraft, type FixMode } from './FixDraft'
import { Pager, usePager } from './Pager'
import { ExtraForm, PayoutForm, SalaryForm, type ExtraDoc, type PayoutDoc, type SalaryDoc } from './PayrollForms'
import { Button, Empty, ErrorBox, Field, Loading, Modal, Money, toast } from './ui'
import { PURPOSE_LABELS } from '../lib/payouts'

interface AccrualRow {
  id: string
  business_date: string
  kind: string
  amount_tyiyn: number
  comment: string
  doc_type: string
  reversal_of: string | null
  reversed: boolean
  user_name: string
  created_at: string
}

interface PayoutRow {
  id: string
  number: number
  amount_tyiyn: number
  /** За что (ADR-122); `general` — прежняя выплата без вида. */
  purpose?: string
  source: string
  account_name: string | null
  comment: string
  reversal_of: string | null
  reversed: boolean
  user_name: string
  created_at: string
}

interface EntriesOut {
  accruals: AccrualRow[]
  payouts: PayoutRow[]
}

const KIND_LABELS: Record<string, string> = {
  monthly_salary: 'Оклад',
  shift_fee: 'За смену',
  bonus: 'Бонус',
  penalty: 'Удержание',
  shortage: 'Недостача',
  revenue_percent: '% с чеков',
  service_fee: 'Мастеру за услуги',
}

type Target = { what: 'accrual'; row: AccrualRow } | { what: 'payout'; row: PayoutRow }

/** Исправляются только ручные бонус и удержание и оклад за месяц; остальное — из чеков и смен. */
const FIXABLE_KINDS = ['bonus', 'penalty', 'monthly_salary']

const MONTHS = ['январь', 'февраль', 'март', 'апрель', 'май', 'июнь', 'июль', 'август', 'сентябрь', 'октябрь', 'ноябрь', 'декабрь']

/** Что отменяем и как это назвать: прощение недостачи — это сторно её удержания (ADR-064). */
function actionLabel(t: Target): string {
  if (t.what === 'payout') return 'Отменить выплату'
  return t.row.kind === 'shortage' ? 'Простить недостачу' : 'Отменить'
}

export function PayrollEntries({ employee, from, to, onClose }: { employee: { employee_id: string; full_name: string }; from: string; to: string; onClose: (changed: boolean) => void }) {
  const data = useLoad(() => get<EntriesOut>(`/payroll/entries${qs({ employee_id: employee.employee_id, from, to })}`), [employee.employee_id, from, to])
  const [target, setTarget] = useState<{ t: Target; reason: string; opId: string } | null>(null)
  const act = useAction()
  const [changed, setChanged] = useState(false)
  const [fixing, setFixing] = useState<Target | null>(null)
  // Отметки «исправлен → / исправление» — одним запросом на список.
  const accMarks = useLoad(() => loadCorrections('accrual').then(correctionMarks), [])
  const payMarks = useLoad(() => loadCorrections('payout').then(correctionMarks), [])

  const fixed = () => {
    setFixing(null)
    setChanged(true)
    data.reload()
    accMarks.reload()
    payMarks.reload()
  }

  const confirm = () =>
    void act.run(async () => {
      if (!target) return
      const { t } = target
      const uri = t.what === 'accrual' ? `/payroll/accruals/${t.row.id}/reverse` : `/payouts/${t.row.id}/reverse`
      await post(uri, { op_id: target.opId, comment: target.reason.trim() })
      toast(t.what === 'accrual' && t.row.kind === 'shortage' ? 'Недостача прощена' : 'Отменено')
      setTarget(null)
      setChanged(true)
      data.reload()
    })

  const done = () => onClose(changed)
  const accruals = data.data?.accruals ?? []
  const payouts = data.data?.payouts ?? []
  const accPager = usePager(accruals, 'payroll.accruals')
  const payPager = usePager(payouts, 'payroll.payouts')
  const struck = (r: { reversed: boolean }) => (r.reversed ? 'text-slate-400 line-through' : '')

  return (
    <Modal title={`${employee.full_name}: начисления и выплаты`} onClose={done} wide>
      <div className="flex flex-col gap-4 text-sm">
        <div className="text-slate-500">
          {from === to ? formatDate(from) : `${formatDate(from)} — ${formatDate(to)}`}. Процент и замены из чеков снимаются возвратом чека, здесь
          их нет. Отмена — новая запись с минусом, исходная остаётся зачёркнутой.
        </div>
        <ErrorBox error={data.error} />
        {fixing ? (
          <div className="flex flex-col gap-3 rounded-md border border-slate-200 p-3">
            <h3 className="font-semibold">
              Исправить: {fixing.what === 'payout' ? `выплата № ${fixing.row.number}` : (KIND_LABELS[fixing.row.kind] ?? fixing.row.kind).toLowerCase()}
            </h3>
            {fixing.what === 'payout' ? (
              <FixDraft<PayoutDoc> key={fixing.row.id} kind="payout" id={fixing.row.id} onCancel={() => setFixing(null)}>
                {(fix) => <PayoutForm employeeId={employee.employee_id} fix={fix} onCancel={() => setFixing(null)} onDone={fixed} />}
              </FixDraft>
            ) : (
              <FixDraft<ExtraDoc | SalaryDoc> key={fixing.row.id} kind="accrual" id={fixing.row.id} onCancel={() => setFixing(null)}>
                {(fix) =>
                  fix.doc.type === 'salary' || !('kind' in fix.doc) ? (
                    <SalaryForm
                      employeeId={employee.employee_id}
                      month={fix.doc.month}
                      hint={`Оклад за ${MONTHS[Number(fix.doc.month.slice(5, 7)) - 1] ?? ''} ${fix.doc.month.slice(0, 4)}.`}
                      fix={fix as FixMode<SalaryDoc>}
                      onCancel={() => setFixing(null)}
                      onDone={fixed}
                    />
                  ) : (
                    <ExtraForm
                      employeeId={employee.employee_id}
                      kind={fix.doc.kind}
                      date={fix.doc.business_date}
                      fix={fix as FixMode<ExtraDoc>}
                      onCancel={() => setFixing(null)}
                      onDone={fixed}
                    />
                  )
                }
              </FixDraft>
            )}
          </div>
        ) : target ? (
          <div className="flex flex-col gap-3 rounded-md border border-rose-200 bg-rose-50/40 p-3 text-sm">
            <h3 className="font-semibold">{actionLabel(target.t)}</h3>
            <div>
              {target.t.what === 'accrual'
                ? target.t.row.kind === 'shortage'
                  ? `Удержание недостачи ${formatSom(-target.t.row.amount_tyiyn)} снимется с кассира. Недостача кассы останется в отчёте о прибыли и уменьшит её на эту сумму.`
                  : `${KIND_LABELS[target.t.row.kind] ?? target.t.row.kind} ${formatSom(target.t.row.amount_tyiyn)} за ${formatDate(target.t.row.business_date)} будет отменён записью с минусом.`
                : `Выплата № ${target.t.row.number}${target.t.row.purpose && target.t.row.purpose !== 'general' ? ` (${(PURPOSE_LABELS[target.t.row.purpose] ?? target.t.row.purpose).toLowerCase()})` : ''} на ${formatSom(target.t.row.amount_tyiyn)} отменится${target.t.row.account_name ? `, деньги вернутся в «${target.t.row.account_name}» (нужна открытая смена)` : ''}.`}
            </div>
            <Field label="Причина" required>
              <input autoFocus value={target.reason} onChange={(e) => setTarget({ ...target, reason: e.target.value })} />
            </Field>
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setTarget(null)}>
                Не отменять
              </Button>
              <Button variant="danger" disabled={act.busy || !target.reason.trim()} onClick={confirm}>
                {actionLabel(target.t)}
              </Button>
            </div>
          </div>
        ) : data.loading && !data.data ? (
          <Loading />
        ) : (
          <>
            <section>
              <h3 className="mb-1 font-semibold">Начисления</h3>
              {accruals.length === 0 ? (
                <Empty>Начислений за период нет</Empty>
              ) : (
                <ul className="divide-y divide-slate-100">
                  {accPager.shown.map((a) => (
                    <li key={a.id} className="flex flex-wrap items-center justify-between gap-2 py-2">
                      <span className={struck(a)}>
                        {formatDate(a.business_date)} · {a.reversal_of ? 'Сторно: ' : ''}
                        {KIND_LABELS[a.kind] ?? a.kind} · <Money value={a.amount_tyiyn} tone="negative" />
                        {a.comment && <span className="text-slate-500"> — {a.comment}</span>} <CorrectionBadges mark={accMarks.data?.get(a.id)} />
                        <span className="block text-xs text-slate-400">
                          {a.user_name}, {formatDateTime(a.created_at)}
                        </span>
                      </span>
                      {a.reversed ? (
                        <span className="text-xs text-slate-500">{a.kind === 'shortage' ? 'прощена' : 'отменено'}</span>
                      ) : (
                        !a.reversal_of && (
                          <span className="flex gap-1">
                            {FIXABLE_KINDS.includes(a.kind) && <FixButton small onClick={() => setFixing({ what: 'accrual', row: a })} />}
                            <Button
                              variant="secondary"
                              className="px-2 py-1 text-xs"
                              onClick={() => setTarget({ t: { what: 'accrual', row: a }, reason: '', opId: newOpId() })}
                            >
                              {actionLabel({ what: 'accrual', row: a })}
                            </Button>
                          </span>
                        )
                      )}
                    </li>
                  ))}
                </ul>
              )}
              <Pager page={accPager.page} size={accPager.size} total={accPager.total} onPage={accPager.setPage} onSize={accPager.setSize} label="Страницы начислений" />
            </section>
            <section>
              <h3 className="mb-1 font-semibold">Выплаты</h3>
              {payouts.length === 0 ? (
                <Empty>Выплат за период нет</Empty>
              ) : (
                <ul className="divide-y divide-slate-100">
                  {payPager.shown.map((p) => (
                    <li key={p.id} className="flex flex-wrap items-center justify-between gap-2 py-2">
                      <span className={struck(p)}>
                        № {p.number} · {formatDateTime(p.created_at)} · {p.reversal_of ? 'Сторно: ' : ''}
                        <Money value={p.amount_tyiyn} tone="negative" />
                        {p.purpose && p.purpose !== 'general' && <> · {(PURPOSE_LABELS[p.purpose] ?? p.purpose).toLowerCase()}</>} ·{' '}
                        {p.account_name ? `из «${p.account_name}»` : 'мимо касс'}
                        {p.comment && <span className="text-slate-500"> — {p.comment}</span>} <CorrectionBadges mark={payMarks.data?.get(p.id)} />
                        <span className="block text-xs text-slate-400">{p.user_name}</span>
                      </span>
                      {p.reversed ? (
                        <span className="text-xs text-slate-500">отменена</span>
                      ) : (
                        !p.reversal_of && (
                          <span className="flex gap-1">
                            <FixButton small onClick={() => setFixing({ what: 'payout', row: p })} />
                            <Button
                              variant="secondary"
                              className="px-2 py-1 text-xs"
                              onClick={() => setTarget({ t: { what: 'payout', row: p }, reason: '', opId: newOpId() })}
                            >
                              Отменить
                            </Button>
                          </span>
                        )
                      )}
                    </li>
                  ))}
                </ul>
              )}
              <Pager page={payPager.page} size={payPager.size} total={payPager.total} onPage={payPager.setPage} onSize={payPager.setSize} label="Страницы выплат" />
            </section>
          </>
        )}
        <div className="flex justify-end">
          <Button variant="secondary" onClick={done}>
            Закрыть
          </Button>
        </div>
      </div>

    </Modal>
  )
}
