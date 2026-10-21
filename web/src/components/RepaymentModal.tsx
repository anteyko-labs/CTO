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
import { invoicePayPreview, repayNote } from '../lib/supplierPay'
import { CorrectionBanner, useCorrection } from './Correction'
import { useTouchMode } from '../lib/cashierMode'
import { Button, Empty, ErrorBox, Field, Loading, Missing, Modal, Table, toast } from './ui'

// «Вне кассы» (из своих) больше нет — сервер отвечает 422 (ADR-079).
type Method = 'cash' | 'card' | 'transfer' | 'invoice'

const METHOD_LABELS: Record<Method, string> = {
  cash: 'Наличные',
  card: 'Карта / QR банка',
  transfer: 'QR владельцу',
  invoice: 'По счёту',
}

const METHOD_HINTS: Record<Method, string> = {
  cash: 'Деньги в кассе смены: смена должна быть открыта.',
  card: 'Карта или официальный QR банка: деньги на безналичный счёт, банк возьмёт комиссию.',
  transfer: 'QR на личный счёт владельца: без комиссии банка.',
  invoice: 'Перечисление юрлица по счёту: на безналичный счёт, без комиссии банка.',
}

/** Поставщику платим мы: деньги уходят с выбранной кассы или счёта (проверка 20.10.2026). */
const SUPPLIER_METHOD_HINTS: Record<Method, string> = {
  cash: 'Наличными из выбранной кассы.',
  card: 'С безналичного счёта (карта / QR банка).',
  transfer: 'С личного счёта владельца (QR владельцу).',
  invoice: 'С безналичного счёта по счёту.',
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
  // Режим кассы: всё крупнее — сумма долга, поле суммы и кнопки (SPEC-30 §13).
  const touch = useTouchMode()
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
    method !== 'cash' ? bankChoices([a]).length > 0 : method === 'cash' && !isBank(a) && (owner || isTill(a)),
  )
  const accountOrdered = method !== 'cash' ? bankChoices(accountOptions, method) : accountOptions
  const accountId = accountOrdered.some((a) => a.id === accountPick) ? accountPick : (accountOrdered[0]?.id ?? '')
  const [comment, setComment] = useState('')
  const [done, setDone] = useState<{ paid: number; before: number; after: number; invoiceBefore?: number; invoiceAfter?: number } | null>(null)
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
  // «По счёту» — перечисление юрлица, без комиссии (ADR-095); поставщику платим как раньше.
  const methods: Method[] = supplier ? (owner ? ['cash', 'card', 'transfer'] : ['cash']) : ['cash', 'card', 'transfer', 'invoice']
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
  // Оплата накладной: остаток накладной и общий долг поставщику — отдельно (проверка 20.10.2026).
  const supplierDebt = details.data?.kind === 'supplier' ? details.data.d.debt_tyiyn : invoice ? null : debt
  const preview = invoice && value !== null ? invoicePayPreview(invoice.left_tyiyn, supplierDebt, value) : null
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
      const after = Math.abs(res.balance_tyiyn)
      setDone(
        invoice
          ? { paid: value, before: supplierDebt ?? debt, after, invoiceBefore: invoice.left_tyiyn, invoiceAfter: Math.max(0, invoice.left_tyiyn - value) }
          : { paid: value, before: debt, after },
      )
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
            {invoice ? (
              <Stat
                label="Всего должны поставщику"
                value={done ? formatSom(done.after) : supplierDebt === null ? '…' : formatSom(supplierDebt)}
                tone="slate"
                testId="repay-debt"
              />
            ) : (
              <Stat label={supplier ? 'Должны мы' : 'Должен нам'} value={formatSom(done ? done.after : debt)} tone={supplier ? 'slate' : 'amber'} testId="repay-debt" big={touch ? 'touch' : true} />
            )}
            {!supplier && (
              <Stat
                label="Просрочено"
                value={overdue > 0 ? formatSom(overdue) : '—'}
                hint={overdue > 0 ? `${overdueDays} дн.` : dueDate ? `срок до ${formatDate(dueDate)}` : 'срок не задан'}
                tone={overdue > 0 ? 'rose' : 'slate'}
              />
            )}
            {invoice && <Stat label={`Осталось по накладной № ${invoice.number}`} value={formatSom(done?.invoiceAfter ?? invoice.left_tyiyn)} tone="slate" />}
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
              {invoice && done.invoiceBefore !== undefined ? (
                <>
                  <div className="text-sm text-slate-700">
                    По накладной № {invoice.number}: было {formatSom(done.invoiceBefore)} — стало{' '}
                    <span className="font-semibold">{formatSom(done.invoiceAfter ?? 0)}</span>
                  </div>
                  <div className="text-sm text-slate-700">
                    Всего должны поставщику: <span className="font-semibold">{formatSom(done.after)}</span>
                  </div>
                </>
              ) : (
                <div className="text-sm text-slate-700">
                  Долг был {formatSom(done.before)} — стал <span className="font-semibold">{formatSom(done.after)}</span>
                </div>
              )}
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
                <input
                  id="repay-sum"
                  className={touch ? 'min-h-[64px] text-2xl!' : 'text-lg'}
                  autoFocus
                  {...(touch ? { inputMode: 'none' as const, 'data-keypad': 'decimal' } : { inputMode: 'decimal' as const })}
                  value={sum}
                  onChange={(e) => setSum(e.target.value)}
                />
              </Field>
              {!invoice && !fixId && value !== null && value > owed && owed > 0 && (
                <div className="text-xs text-amber-700">Больше долга: разница станет {supplier ? 'нашим авансом поставщику' : 'авансом клиента'}.</div>
              )}
              {!fixId && invoice && preview && value !== null && value > 0 && (
                <div className="text-sm text-slate-600" data-testid="repay-after">
                  По накладной останется: <span className="font-semibold tabular-nums">{formatSom(preview.invoiceAfter)}</span>
                  {preview.supplierAfter !== null && (
                    <>
                      {' · '}всего поставщику: <span className="font-semibold tabular-nums">{formatSom(preview.supplierAfter)}</span>
                    </>
                  )}
                </div>
              )}
              {!fixId && !invoice && value !== null && value > 0 && (
                <div className="text-sm text-slate-600" data-testid="repay-after">
                  После оплаты долг: <span className="font-semibold tabular-nums">{formatSom(Math.max(0, debt - value))}</span>
                  {/* Переплата видна суммой: она станет авансом (проверка 20.10.2026). */}
                  {value > debt && debt >= 0 && (
                    <span className="ml-1 text-amber-700">
                      · переплата {formatSom(value - debt)} — {supplier ? 'наш аванс поставщику' : 'аванс клиента'}
                    </span>
                  )}
                </div>
              )}
              <div className="text-sm font-medium text-slate-700">Способ оплаты</div>
              <div className={`grid overflow-hidden rounded-md border border-slate-300 text-sm`} style={{ gridTemplateColumns: `repeat(${methods.length}, minmax(0, 1fr))` }}>
                {methods.map((m) => (
                  <button
                    key={m}
                    type="button"
                    className={`${touch ? 'min-h-[64px] text-lg' : 'min-h-[42px]'} py-2 ${method === m ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
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
                {(supplier ? SUPPLIER_METHOD_HINTS : METHOD_HINTS)[method]} {repayNote(supplier)}
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
                  <Button className={touch ? 'min-h-[64px]! px-8 text-xl!' : ''} disabled={busy || notFilled.length > 0} onClick={save}>
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

function Stat({
  label,
  value,
  hint,
  tone,
  testId,
  big,
}: {
  label: string
  value: string
  hint?: string
  tone: 'amber' | 'rose' | 'slate'
  testId?: string
  /** Сумма долга крупно; в режиме кассы — ещё крупнее. */
  big?: boolean | 'touch'
}) {
  const color = tone === 'amber' ? 'text-amber-700' : tone === 'rose' ? 'text-rose-700' : 'text-slate-900'
  const size = big === 'touch' ? 'text-4xl' : big ? 'text-3xl' : 'text-xl'
  return (
    <div className="rounded-md border border-slate-200 bg-slate-50 p-3">
      <div className="text-xs uppercase text-slate-500">{label}</div>
      <div className={`${size} font-bold tabular-nums ${color}`} data-testid={testId}>
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

/**
 * Касса: найти должника и принять от него оплату. Окно крупное — кассир выбирает пальцем
 * (SPEC-30 §13); клиент чека с долгом — окно сразу открывается на нём, без поиска.
 */
export function DebtorPayment({ party: chequeParty, onClose, onPaid }: { party?: Party | null; onClose: () => void; onPaid?: () => void }) {
  const touch = useTouchMode()
  const [q, setQ] = useState('')
  const query = useDebounced(q)
  // Долг клиента чека — свежий, с сервера: в чеке он мог остаться с прошлого раза.
  const current = useLoad(
    () => (chequeParty?.role === 'customer' ? get<Party>(`/parties/${chequeParty.id}`) : Promise.resolve(null)),
    [chequeParty?.id, chequeParty?.role],
  )
  const list = useLoad(() => get<Party[]>(`/parties${qs({ role: 'customer', owes: true, q: query })}`), [query])
  const [party, setParty] = useState<Party | null>(null)
  // Клиента чека выбрали и закрыли оплату — дальше обычный поиск, а не снова он.
  const [skipCurrent, setSkipCurrent] = useState(false)
  const debtors = (list.data ?? []).filter((p) => p.balance_tyiyn > 0)
  const done = () => {
    onPaid?.()
    onClose()
  }

  const chequeDebtor = !skipCurrent && current.data && current.data.balance_tyiyn > 0 ? current.data : null
  const chosen = party ?? chequeDebtor
  if (chosen) {
    return (
      <RepaymentModal
        party={chosen}
        onClose={() => {
          // Из окна клиента чека — назад к поиску; выбранного из списка — тоже к списку.
          setParty(null)
          setSkipCurrent(true)
        }}
        onDone={done}
      />
    )
  }
  if (chequeParty && current.loading && !current.data && !current.error) {
    return (
      <Modal title="Оплата долга" onClose={onClose}>
        <Loading />
      </Modal>
    )
  }
  const row = touch ? 'min-h-[72px] px-4 py-3 text-xl' : 'min-h-[60px] px-3 py-2 text-lg'
  return (
    <Modal title="Оплата долга" full onClose={onClose}>
      <div className="flex flex-col gap-3">
        {chequeParty && current.data && current.data.balance_tyiyn <= 0 && (
          <div className="rounded-md bg-slate-50 px-3 py-2 text-sm text-slate-600" data-testid="cheque-party-no-debt">
            У клиента чека «{current.data.name}» долга нет.
          </div>
        )}
        <input
          autoFocus
          className={touch ? 'min-h-[64px] text-xl' : 'min-h-[48px] text-lg'}
          placeholder="Имя, телефон или ИНН должника"
          value={q}
          onChange={(e) => setQ(e.target.value)}
        />
        <ErrorBox error={list.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : list.error ? null : debtors.length === 0 ? (
          <Empty>{query ? 'Должник не найден' : 'Должников нет'}</Empty>
        ) : (
          <ul className="flex max-h-[65vh] flex-col divide-y divide-slate-100 overflow-y-auto rounded-md border border-slate-200" data-testid="debtors">
            {debtors.map((p) => (
              <li key={p.id}>
                <button type="button" className={`flex w-full items-center justify-between gap-4 text-left hover:bg-slate-50 active:bg-sky-50 ${row}`} onClick={() => setParty(p)}>
                  <span className="min-w-0">
                    <span className="block truncate font-medium">{p.name}</span>
                    {p.phone && <span className={`block text-slate-500 ${touch ? 'text-base' : 'text-sm'}`}>{p.phone}</span>}
                  </span>
                  <span className={`shrink-0 font-bold tabular-nums text-rose-700 ${touch ? 'text-3xl' : 'text-2xl'}`}>{formatSom(p.balance_tyiyn)}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </Modal>
  )
}
