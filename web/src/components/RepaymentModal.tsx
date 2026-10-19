// Оплата долга: клиент гасит нам или мы платим поставщику, деньги проходят через кассу (SPEC-10).
// Большое окно (SPEC-28 §3): долг, просрочка, неоплаченные чеки или накладные, оплата и итог после неё.
import { useState } from 'react'
import { get, newOpId, post, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatDate, formatSom, parseSom, somInput } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useDebounced, useLoad } from '../lib/hooks'
import type { CashAccount, OpenDebt, OpenDebts, Party, SupplierInvoice, SupplierInvoices } from '../lib/types'
import { accountLabel, bankChoices, isBank, isTill } from '../lib/accounts'
import { repaymentDoc, repaymentFields, type RepaymentDoc } from '../lib/moneyFix'
import { CorrectionBanner, useCorrection } from './Correction'
import { Button, Empty, ErrorBox, Field, Loading, Missing, Modal, Table, toast } from './ui'

// «Вне кассы» (из своих) больше нет — сервер отвечает 422 (ADR-079).
type Method = 'cash' | 'card' | 'transfer'

const METHOD_LABELS: Record<Method, string> = {
  cash: 'Наличные',
  card: 'Карта',
  transfer: 'QR',
}

const METHOD_HINTS: Record<Method, string> = {
  cash: 'Деньги в кассе смены: смена должна быть открыта.',
  card: 'Оплата картой через терминал: деньги на безналичный счёт, банк возьмёт комиссию.',
  transfer: 'Оплата QR через терминал: деньги на безналичный счёт, банк возьмёт комиссию.',
}

/** Накладная, которую оплачиваем: сумма не больше её остатка (SPEC-27). */
export interface InvoiceToPay {
  receipt_id: string
  number: number
  left_tyiyn: number
}

export function RepaymentModal({
  party,
  invoice,
  fixId,
  onClose,
  onDone,
}: {
  party: Pick<Party, 'id' | 'name' | 'balance_tyiyn' | 'role'>
  /** Оплата одной накладной; без неё оплата закрывает самые старые накладные (ADR-073). */
  invoice?: InvoiceToPay
  /** «Исправить» проведённую оплату (SPEC-29 §24): id записи долга; окно заполняется ею. */
  fixId?: string
  onClose: () => void
  onDone: (balance: number) => void
}) {
  const owner = useUser().role === 'owner'
  const supplier = party.role === 'supplier'
  // Что должен и за что: у клиента — неоплаченные чеки, у поставщика — открытые накладные (SPEC-28 §3).
  const details = useLoad<{ kind: 'supplier'; d: SupplierInvoices } | { kind: 'customer'; d: OpenDebts }>(
    () =>
      supplier
        ? get<SupplierInvoices>(`/parties/${party.id}/invoices`).then((d) => ({ kind: 'supplier' as const, d }))
        : get<OpenDebts>(`/parties/${party.id}/open-debts`).then((d) => ({ kind: 'customer' as const, d })),
    [party.id, supplier],
  )
  const owed = invoice ? invoice.left_tyiyn : Math.abs(party.balance_tyiyn)
  const [sum, setSum] = useState(owed > 0 ? somInput(owed) : '')
  const [method, setMethod] = useState<Method>('cash')
  // Касса или счёт (ADR-076): карта и QR — безналичный счёт (по умолчанию — свой для способа,
  // ADR-077), наличные у владельца — любая касса.
  const accounts = useLoad(() => get<CashAccount[]>('/cash/accounts'), [])
  const [accountPick, setAccountPick] = useState('')
  const accountOptions = (accounts.data ?? []).filter((a) =>
    method === 'card' || method === 'transfer' ? bankChoices([a]).length > 0 : method === 'cash' && !isBank(a) && (owner || isTill(a)),
  )
  const accountOrdered = method === 'card' || method === 'transfer' ? bankChoices(accountOptions, method) : accountOptions
  const accountId = accountOrdered.some((a) => a.id === accountPick) ? accountPick : (accountOrdered[0]?.id ?? '')
  const [comment, setComment] = useState('')
  const [done, setDone] = useState<{ paid: number; before: number; after: number } | null>(null)
  const { busy, error, run } = useAction()
  // Один op_id на открытую форму: повтор после потерянного ответа не задвоит оплату.
  const [opId] = useState(newOpId)
  // Исправление: поля — из исходной оплаты, как только она пришла с сервера.
  const correction = useCorrection<RepaymentDoc>('ledger', fixId ?? null)
  const fixDraft = fixId ? correction.draft.data : null
  const [filled, setFilled] = useState(false)
  if (fixDraft && !filled) {
    const f = repaymentFields(fixDraft.doc)
    setSum(f.sum)
    setMethod(f.method)
    setAccountPick(f.account)
    setComment(f.comment)
    setFilled(true)
  }
  // Администратор поставщику платит только из кассы смены.
  const methods: Method[] = owner || !supplier ? ['cash', 'card', 'transfer'] : ['cash']
  const value = parseSom(sum)
  const notFilled = missingWithFocus(
    [value !== null && value > 0, 'сумму', '#repay-sum'],
    [!invoice || (value !== null && value <= invoice.left_tyiyn), 'сумму не больше остатка накладной', '#repay-sum'],
    [!fixId || Boolean(correction.comment.trim()), 'что и почему исправляете', '#correction-comment'],
  )
  const debt = Math.abs(party.balance_tyiyn)
  const overdue = details.data?.kind === 'customer' ? details.data.d.overdue_tyiyn : 0
  const overdueDays = details.data?.kind === 'customer' ? details.data.d.overdue_days : 0
  const dueDate = details.data?.kind === 'customer' ? details.data.d.due_date : null
  // После оплаты окно показывает итог; закрытие любым способом обновляет экран под ним.
  const close = () => (done ? onDone(supplier ? -done.after : done.after) : onClose())

  const save = () =>
    void run(async () => {
      if (value === null) return
      const res = await post<{ balance_tyiyn: number }>('/debts/repayments', {
        op_id: opId,
        party_id: party.id,
        amount_tyiyn: value,
        method,
        account_id: accountId || undefined,
        comment,
        receipt_id: invoice?.receipt_id,
      })
      toast(
        invoice
          ? `Накладная № ${invoice.number}: оплачено ${formatSom(value)}`
          : supplier
            ? 'Оплата поставщику записана'
            : `Оплата долга принята: ${formatSom(value)}`,
      )
      setDone({ paid: value, before: debt, after: Math.abs(res.balance_tyiyn) })
      details.reload()
    })

  const saveFix = async () => {
    if (!fixDraft) return
    const doc = repaymentDoc(fixDraft.doc, { sum, method, account: accountId, comment })
    if (!doc) return
    const c = await correction.save(doc)
    if (!c) return
    toast(`Исправлено: ${supplier ? 'оплата поставщику' : 'оплата долга'} ${formatSom(doc.amount_tyiyn)}`)
    onDone(party.balance_tyiyn)
  }

  const title = fixId ? `Исправить оплату: ${party.name}` : invoice ? `Оплата накладной № ${invoice.number}: ${party.name}` : supplier ? `Оплата поставщику: ${party.name}` : `Оплата долга: ${party.name}`

  return (
    <Modal title={title} full onClose={close}>
      <div className="grid gap-6 lg:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
        <section className="flex min-w-0 flex-col gap-4" aria-label="Долг">
          <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
            <Stat label={supplier ? 'Должны мы' : 'Должен нам'} value={formatSom(done ? done.after : debt)} tone={supplier ? 'slate' : 'amber'} testId="repay-debt" />
            {!supplier && (
              <Stat
                label="Просрочено"
                value={overdue > 0 ? formatSom(overdue) : '—'}
                hint={overdue > 0 ? `${overdueDays} дн.` : dueDate ? `срок до ${formatDate(dueDate)}` : 'срок не задан'}
                tone={overdue > 0 ? 'rose' : 'slate'}
              />
            )}
            {invoice && <Stat label="Осталось по накладной" value={formatSom(invoice.left_tyiyn)} tone="slate" />}
          </div>
          <div>
            <h3 className="mb-2 text-sm font-semibold text-slate-700">{supplier ? 'Неоплаченные накладные' : 'Неоплаченные чеки'}</h3>
            <ErrorBox error={details.error} />
            {details.loading && !details.data ? (
              <Loading />
            ) : details.data?.kind === 'supplier' ? (
              <SupplierOpen invoices={details.data.d.invoices.filter((i) => i.left_tyiyn > 0)} current={invoice?.receipt_id} />
            ) : details.data?.kind === 'customer' ? (
              <CustomerOpen items={details.data.d.items} />
            ) : null}
          </div>
        </section>

        <section className="flex min-w-0 flex-col gap-3" aria-label="Оплата">
          {done ? (
            <div className="flex flex-col gap-3 rounded-md border border-emerald-200 bg-emerald-50 p-4" data-testid="repay-result">
              <div className="text-lg font-semibold text-emerald-800">
                {supplier ? 'Оплачено' : 'Принято'} {formatSom(done.paid)}
              </div>
              <div className="text-sm text-slate-700">
                Долг был {formatSom(done.before)} — стал <span className="font-semibold">{formatSom(done.after)}</span>
              </div>
              <div className="text-xs text-slate-500">{METHOD_LABELS[method]}{comment.trim() ? ` · ${comment.trim()}` : ''}</div>
              <div className="flex justify-end">
                <Button onClick={close}>Готово</Button>
              </div>
            </div>
          ) : (
            <>
              {fixDraft && (
                <CorrectionBanner label={fixDraft.kind_label} number={fixDraft.number} comment={correction.comment} onComment={correction.setComment} />
              )}
              {fixId && !filled && correction.draft.loading && <Loading />}
              {supplier && !invoice && !fixId && <div className="text-xs text-slate-500">Оплата закрывает самые старые неоплаченные накладные.</div>}
              {!supplier && !fixId && <div className="text-xs text-slate-500">Оплата гасит самые старые долги первыми.</div>}
              <Field label="Сумма, с" required>
                <input id="repay-sum" className="text-lg" autoFocus inputMode="decimal" value={sum} onChange={(e) => setSum(e.target.value)} />
              </Field>
              {!invoice && !fixId && value !== null && value > owed && owed > 0 && (
                <div className="text-xs text-amber-700">Больше долга: разница станет {supplier ? 'нашим авансом поставщику' : 'авансом клиента'}.</div>
              )}
              {!fixId && value !== null && value > 0 && (
                <div className="text-sm text-slate-600" data-testid="repay-after">
                  После оплаты долг: <span className="font-semibold tabular-nums">{formatSom(Math.max(0, debt - value))}</span>
                </div>
              )}
              <div className="text-sm font-medium text-slate-700">Способ оплаты</div>
              <div className={`grid overflow-hidden rounded-md border border-slate-300 text-sm`} style={{ gridTemplateColumns: `repeat(${methods.length}, minmax(0, 1fr))` }}>
                {methods.map((m) => (
                  <button
                    key={m}
                    type="button"
                    className={`min-h-[42px] py-2 ${method === m ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
                    onClick={() => {
                      setMethod(m)
                      // Новый способ — снова его счёт по умолчанию.
                      setAccountPick('')
                    }}
                  >
                    {METHOD_LABELS[m]}
                  </button>
                ))}
              </div>
              {accountOrdered.length > 1 && (
                <Field label={method === 'cash' ? 'Касса' : 'Безналичный счёт'}>
                  <select id="repay-account" value={accountId} onChange={(e) => setAccountPick(e.target.value)}>
                    {accountOrdered.map((a) => (
                      <option key={a.id} value={a.id}>
                        {accountLabel(a)}
                      </option>
                    ))}
                  </select>
                </Field>
              )}
              {/* Один счёт на выбор (администратору — касса смены): остаток всё равно виден (SPEC-29 §12). */}
              {accountOrdered.length === 1 && (
                <div className="text-sm text-slate-600" data-testid="repay-account-one">
                  {supplier ? 'Платим из: ' : 'Деньги в: '}
                  <b>{accountLabel(accountOrdered[0])}</b>
                </div>
              )}
              <div className="text-xs text-slate-500">
                {METHOD_HINTS[method]} В прибыль оплата долга не идёт: выручка и прибыль по чеку учтены в день продажи.
              </div>
              <Field label="Комментарий">
                <input value={comment} onChange={(e) => setComment(e.target.value)} />
              </Field>
              <Missing items={notFilled} />
              <ErrorBox error={fixId ? correction.error : error} />
              <div className="flex justify-end gap-2">
                <Button variant="secondary" onClick={onClose}>
                  Отмена
                </Button>
                {fixId ? (
                  <Button disabled={correction.busy || !filled || notFilled.length > 0} onClick={() => void saveFix()}>
                    Сохранить исправление
                  </Button>
                ) : (
                  <Button disabled={busy || notFilled.length > 0} onClick={save}>
                    {supplier ? 'Оплатить' : 'Принять оплату'}
                  </Button>
                )}
              </div>
            </>
          )}
        </section>
      </div>
    </Modal>
  )
}

function Stat({ label, value, hint, tone, testId }: { label: string; value: string; hint?: string; tone: 'amber' | 'rose' | 'slate'; testId?: string }) {
  const color = tone === 'amber' ? 'text-amber-700' : tone === 'rose' ? 'text-rose-700' : 'text-slate-900'
  return (
    <div className="rounded-md border border-slate-200 bg-slate-50 p-3">
      <div className="text-xs uppercase text-slate-500">{label}</div>
      <div className={`text-xl font-bold tabular-nums ${color}`} data-testid={testId}>
        {value}
      </div>
      {hint && <div className="text-xs text-slate-500">{hint}</div>}
    </div>
  )
}

function CustomerOpen({ items }: { items: OpenDebt[] }) {
  if (items.length === 0) return <Empty>Неоплаченных чеков нет</Empty>
  return (
    <div className="max-h-[50vh] overflow-y-auto">
      <Table head={['Дата', 'Документ', 'Сумма', 'Осталось']}>
        {items.map((d) => (
          <tr key={d.ledger_id} className={d.overdue ? 'bg-rose-50' : ''}>
            <td className="whitespace-nowrap px-2 py-2">
              {formatDate(d.business_date)}
              {d.overdue && <span className="ml-1 text-xs font-medium text-rose-700">просрочен</span>}
            </td>
            <td className="px-2 py-2">{d.sale_number !== null ? `Чек № ${d.sale_number}` : d.comment || (d.kind === 'adjust' ? 'Правка долга' : 'Долг')}</td>
            <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">{formatSom(d.amount_tyiyn)}</td>
            <td className="whitespace-nowrap px-2 py-2 text-right font-semibold tabular-nums">{formatSom(d.left_tyiyn)}</td>
          </tr>
        ))}
      </Table>
    </div>
  )
}

function SupplierOpen({ invoices, current }: { invoices: SupplierInvoice[]; current?: string }) {
  if (invoices.length === 0) return <Empty>Неоплаченных накладных нет</Empty>
  return (
    <div className="max-h-[50vh] overflow-y-auto">
      <Table head={['Дата', 'Накладная', 'Сумма', 'Оплачено', 'Осталось']}>
        {invoices.map((i) => (
          <tr key={i.ledger_id} className={i.receipt_id && i.receipt_id === current ? 'bg-sky-50' : ''}>
            <td className="whitespace-nowrap px-2 py-2">{formatDate(i.business_date)}</td>
            <td className="px-2 py-2">
              {i.number !== null ? `№ ${i.number}` : i.comment || 'Долг'}
              {i.supplier_doc && <span className="ml-1 text-xs text-slate-500">({i.supplier_doc})</span>}
            </td>
            <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">{formatSom(i.total_tyiyn)}</td>
            <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">{formatSom(i.paid_tyiyn)}</td>
            <td className="whitespace-nowrap px-2 py-2 text-right font-semibold tabular-nums">{formatSom(i.left_tyiyn)}</td>
          </tr>
        ))}
      </Table>
    </div>
  )
}

/** Касса: найти должника и принять от него оплату. */
export function DebtorPayment({ onClose }: { onClose: () => void }) {
  const [q, setQ] = useState('')
  const query = useDebounced(q)
  const list = useLoad(() => get<Party[]>(`/parties${qs({ role: 'customer', only_debtors: true, q: query })}`), [query])
  const [party, setParty] = useState<Party | null>(null)
  const debtors = (list.data ?? []).filter((p) => p.balance_tyiyn > 0)

  if (party) return <RepaymentModal party={party} onClose={onClose} onDone={onClose} />
  return (
    <Modal title="Оплата долга" onClose={onClose}>
      <div className="flex flex-col gap-3">
        <input autoFocus placeholder="Имя, телефон или ИНН должника" value={q} onChange={(e) => setQ(e.target.value)} />
        <ErrorBox error={list.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : debtors.length === 0 ? (
          <Empty>{query ? 'Должник не найден' : 'Должников нет'}</Empty>
        ) : (
          <ul className="flex max-h-80 flex-col divide-y divide-slate-100 overflow-y-auto">
            {debtors.map((p) => (
              <li key={p.id}>
                <button type="button" className="flex w-full items-center justify-between gap-3 px-2 py-2 text-left hover:bg-slate-50" onClick={() => setParty(p)}>
                  <span>
                    <span className="font-medium">{p.name}</span>
                    {p.phone && <span className="ml-2 text-xs text-slate-500">{p.phone}</span>}
                  </span>
                  <span className="font-semibold text-rose-700">{formatSom(p.balance_tyiyn)}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </Modal>
  )
}
