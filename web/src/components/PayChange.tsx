/**
 * «Изменить оплату» чека и «из наличных в QR» за смену (SPEC-37, ADR-132). Только владелец и
 * только при связи с сервером. «Что изменится» сервер считает пробным проведением.
 */
import { useEffect, useMemo, useState } from 'react'
import { Link } from 'react-router-dom'
import { get, newOpId, post, qs } from '../lib/api'
import { useAuth } from '../lib/auth'
import { formatDateTime, formatSom, parseSom } from '../lib/format'
import { useAction, useDebounced, useLoad, useServerOnline } from '../lib/hooks'
import {
  diffWords,
  formPayments,
  initialForm,
  matchingQr,
  remaining,
  type AfterClose,
  type CashToBankInfo,
  type CashToBankOut,
  type Effect,
  type PartyBrief,
  type PayChangeInfo,
  type PayChangeOut,
  type PayForm,
  type TransferRow,
} from '../lib/payChange'
import { PAYMENT_LABELS, type Party } from '../lib/types'
import { Button, ErrorBox, Field, Loading, Modal, Notice } from './ui'

/** Кнопка видна владельцу при связи с сервером. */
function useOwnerOnline(): boolean {
  const { user } = useAuth()
  const online = useServerOnline()
  return user?.role === 'owner' && online
}

export function PayChangeButton({ onClick }: { onClick: () => void }) {
  if (!useOwnerOnline()) return null
  return (
    <Button variant="secondary" onClick={onClick} data-testid="pay-change-button">
      Изменить оплату
    </Button>
  )
}

/** Блок «Что изменится»: строки сервера и галочка удержания. */
function EffectBox({
  effect,
  fix,
  onFix,
}: {
  effect: Effect | null
  fix: boolean
  onFix: (v: boolean) => void
}) {
  if (!effect) return null
  return (
    <Notice tone="sky">
      <div className="flex flex-col gap-1 text-sm" data-testid="pay-effect">
        <div className="font-semibold">Что изменится</div>
        {effect.lines.map((l, i) => (
          <div key={i}>{l}</div>
        ))}
        {effect.counted?.at === 'close' && effect.withhold_possible_tyiyn !== 0 && (
          <label className="mt-1 flex items-center gap-2">
            <input type="checkbox" checked={fix} onChange={(e) => onFix(e.target.checked)} data-testid="fix-withholding" />
            Поправить удержание кассира
          </label>
        )}
      </div>
    </Notice>
  )
}

function shiftLabel(info: PayChangeInfo): string {
  if (info.counted) return `смена № ${info.counted.number} · ${info.counted.open_now ? 'открыта' : 'закрыта'}`
  if (info.open_shift_number != null) return `смена № ${info.open_shift_number} · открыта`
  return 'без смены'
}

/** Клиент для долга: клиент чека или существующий, найденный поиском. */
function PartySearch({ value, onPick }: { value: PartyBrief | null; onPick: (p: PartyBrief | null) => void }) {
  const [q, setQ] = useState('')
  const query = useDebounced(q.trim(), 250)
  const found = useLoad(
    () => (query.length < 2 ? Promise.resolve<Party[]>([]) : get<Party[]>(`/parties${qs({ role: 'customer', q: query, limit: 8 })}`)),
    [query],
  )
  if (value)
    return (
      <div className="flex items-center gap-2 text-sm">
        <span className="font-medium">{value.name}</span>
        <Button variant="secondary" className="min-h-0 px-2 py-1 text-xs" onClick={() => onPick(null)}>
          Другой
        </Button>
      </div>
    )
  return (
    <div className="flex flex-col gap-1">
      <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="имя, телефон или ИНН" data-testid="pay-party-search" />
      {(found.data ?? []).map((p) => (
        <button
          key={p.id}
          type="button"
          className="rounded border border-slate-200 px-2 py-1 text-left text-sm hover:bg-slate-50"
          onClick={() => onPick({ id: p.id, name: p.name, kind: p.kind, inn: p.inn, balance_tyiyn: p.balance_tyiyn })}
        >
          {p.name} {p.phone ? `· ${p.phone}` : ''}
        </button>
      ))}
    </div>
  )
}

/** Окно «Изменить оплату» чека (SPEC-37 §2). Сохранили — открывается новый чек. */
export function PayChangeModal({ saleId, onClose, onSaved }: { saleId: string; onClose: () => void; onSaved: (newId: string) => void }) {
  const info = useLoad(() => get<PayChangeInfo>(`/sales/${saleId}/pay-change`), [saleId])
  if (!info.data) {
    return (
      <Modal title="Изменить оплату" onClose={onClose}>
        {info.error ? <ErrorBox error={info.error} /> : <Loading />}
      </Modal>
    )
  }
  return <PayChangeForm info={info.data} onClose={onClose} onSaved={onSaved} />
}

function PayChangeForm({ info, onClose, onSaved }: { info: PayChangeInfo; onClose: () => void; onSaved: (newId: string) => void }) {
  const [form, setForm] = useState<PayForm>(() => initialForm(info))
  const [party, setParty] = useState<PartyBrief | null>(info.party)
  const [inn, setInn] = useState('')
  const [comment, setComment] = useState('')
  const [fix, setFix] = useState(true)
  const [opId] = useState(newOpId)
  const [effect, setEffect] = useState<Effect | null>(null)
  const preview = useAction()
  const save = useAction()
  const set = (k: keyof PayForm) => (v: string) => setForm((f) => ({ ...f, [k]: v }))

  const lines = formPayments(form)
  const rest = remaining(info.to_split_tyiyn, lines)
  const debt = lines?.find((l) => l.method === 'debt')?.amount_tyiyn ?? 0
  const card = lines?.find((l) => l.method === 'card')?.amount_tyiyn ?? 0
  const qr = matchingQr(info.qr, card)
  const company = party?.kind === 'company'
  const needInn = debt > 0 && party != null && !party.inn.trim()
  const body = useMemo(
    () => ({
      payments: lines ?? [],
      party_id: debt > 0 && party && party.id !== info.party?.id ? party.id : undefined,
      party_inn: needInn && inn.trim() ? inn.trim() : undefined,
      fix_withholding: fix,
    }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [JSON.stringify(lines), party?.id, inn, fix, debt, needInn],
  )
  const ready = rest === 0 && (debt === 0 || (party != null && (!needInn || inn.trim() !== '')))
  const key = useDebounced(ready ? JSON.stringify(body) : '', 300)

  useEffect(() => {
    if (!key) {
      setEffect(null)
      return
    }
    void preview.run(async () => {
      const out = await post<PayChangeOut>(`/sales/${info.sale_id}/pay-change`, { ...body, op_id: newOpId(), comment, dry_run: true })
      setEffect(out.effect)
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key])

  const submit = () =>
    void save.run(async () => {
      const out = await post<PayChangeOut>(`/sales/${info.sale_id}/pay-change`, { ...body, op_id: opId, comment: comment.trim() })
      if (out.new_sale_id) onSaved(out.new_sale_id)
    })

  const sumInput = (k: 'cash' | 'card' | 'transfer' | 'invoice' | 'debt') => (
    <input inputMode="decimal" value={form[k]} onChange={(e) => set(k)(e.target.value)} data-testid={`pay-${k}`} className="w-32" />
  )
  const accountSelect = (k: 'cardAccount' | 'transferAccount' | 'invoiceAccount') => (
    <select value={form[k]} onChange={(e) => set(k)(e.target.value)} data-testid={`pay-${k}`}>
      {info.accounts.map((a) => (
        <option key={a.id} value={a.id}>
          {a.name}
        </option>
      ))}
    </select>
  )

  return (
    <Modal title={`Изменить оплату · чек № ${info.number}`} onClose={onClose} wide>
      <div className="flex flex-col gap-3 text-sm" data-testid="pay-change">
        <div className="text-slate-600">
          {info.business_date} · {formatDateTime(info.created_at)} · кассир {info.cashier_name} · итог {formatSom(info.total_tyiyn)} ·{' '}
          {shiftLabel(info)}
        </div>
        <div>
          <div className="font-semibold">Оплата сейчас</div>
          {info.payments.map((p, i) => (
            <div key={i}>
              {PAYMENT_LABELS[p.method]}
              {p.account_name ? ` (${p.account_name})` : ''}: {formatSom(p.amount_tyiyn)}
            </div>
          ))}
          {info.fixed.map((p, i) => (
            <div key={`f${i}`} className="text-slate-500">
              {PAYMENT_LABELS[p.method]}: {formatSom(p.amount_tyiyn)} — не меняются
            </div>
          ))}
        </div>
        <div className="flex flex-col gap-2">
          <div className="font-semibold">Новая оплата</div>
          <div className="flex flex-wrap items-center gap-2">
            <span className="w-36">{PAYMENT_LABELS.cash}</span>
            {sumInput('cash')}
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <span className="w-36">{PAYMENT_LABELS.card}</span>
            {sumInput('card')}
            {info.accounts.length > 0 && accountSelect('cardAccount')}
          </div>
          {qr.length > 0 && (
            <div className="flex flex-wrap items-center gap-2 pl-36">
              <select value={form.qr} onChange={(e) => set('qr')(e.target.value)} data-testid="pay-qr">
                <option value="">QR не привязывать</option>
                {qr.map((q) => (
                  <option key={q.id} value={q.id}>
                    Привязать QR {formatSom(q.amount_tyiyn)}
                    {q.paid_at ? `, ${formatDateTime(q.paid_at)}` : ''}
                  </option>
                ))}
              </select>
            </div>
          )}
          <div className="flex flex-wrap items-center gap-2">
            <span className="w-36">{PAYMENT_LABELS.transfer}</span>
            {sumInput('transfer')}
            {info.accounts.length > 0 && accountSelect('transferAccount')}
          </div>
          {company && (
            <div className="flex flex-wrap items-center gap-2">
              <span className="w-36">{PAYMENT_LABELS.invoice}</span>
              {sumInput('invoice')}
              {info.accounts.length > 0 && accountSelect('invoiceAccount')}
            </div>
          )}
          <div className="flex flex-wrap items-center gap-2">
            <span className="w-36">{PAYMENT_LABELS.debt}</span>
            {sumInput('debt')}
          </div>
          {debt > 0 && (
            <div className="flex flex-col gap-2 pl-36">
              {info.party ? <div>Клиент: {info.party.name}</div> : <PartySearch value={party} onPick={setParty} />}
              {needInn && (
                <Field label="ПИН или ИНН клиента" required>
                  <input value={inn} onChange={(e) => setInn(e.target.value)} maxLength={14} data-testid="pay-inn" />
                </Field>
              )}
            </div>
          )}
          {info.repaid_tyiyn > 0 && <Notice>По чеку уже погашено {formatSom(info.repaid_tyiyn)} — станет авансом клиента.</Notice>}
          {rest != null && rest !== 0 && (
            <div className="font-medium text-amber-700" data-testid="pay-rest">
              Осталось распределить: {formatSom(rest)}
            </div>
          )}
          {rest == null && <div className="text-rose-700">Неверная сумма</div>}
        </div>
        <Field label="Что и почему меняете" required>
          <input value={comment} maxLength={500} onChange={(e) => setComment(e.target.value)} data-testid="pay-comment" />
        </Field>
        <ErrorBox error={preview.error} />
        <EffectBox effect={effect} fix={fix} onFix={setFix} />
        <ErrorBox error={save.error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={!ready || !comment.trim() || save.busy} onClick={submit} data-testid="pay-save">
            Сохранить
          </Button>
        </div>
      </div>
    </Modal>
  )
}

/** Кнопка «Из наличных в QR» в отчёте смены. */
export function CashToBankButton({ onClick }: { onClick: () => void }) {
  if (!useOwnerOnline()) return null
  return (
    <Button variant="secondary" onClick={onClick} data-testid="cash-to-bank-button">
      Из наличных в QR
    </Button>
  )
}

/** Окно «Из наличных в QR» за смену (SPEC-37 §3). */
export function CashToBankModal({ shiftId, onClose, onDone }: { shiftId: string; onClose: () => void; onDone: () => void }) {
  const info = useLoad(() => get<CashToBankInfo>(`/shifts/${shiftId}/cash-to-bank`), [shiftId])
  const d = info.data
  if (!d) {
    return (
      <Modal title="Из наличных в QR" onClose={onClose}>
        {info.error ? <ErrorBox error={info.error} /> : <Loading />}
      </Modal>
    )
  }
  return <CashToBankForm info={d} onClose={onClose} onDone={onDone} />
}

function CashToBankForm({ info, onClose, onDone }: { info: CashToBankInfo; onClose: () => void; onDone: () => void }) {
  const [amount, setAmount] = useState('')
  const [account, setAccount] = useState((info.accounts.find((a) => a.default_card) ?? info.accounts[0])?.id ?? '')
  const [comment, setComment] = useState('')
  const [fix, setFix] = useState(true)
  const [opId] = useState(newOpId)
  const [effect, setEffect] = useState<Effect | null>(null)
  const preview = useAction()
  const save = useAction()
  const value = parseSom(amount.trim())
  const valid = value != null && value > 0 && value <= info.limit_tyiyn && account !== ''
  const key = useDebounced(valid ? `${value}|${account}|${fix}` : '', 300)
  const body = { amount_tyiyn: value ?? 0, account_id: account, fix_withholding: fix }

  useEffect(() => {
    if (!key) {
      setEffect(null)
      return
    }
    void preview.run(async () => {
      const out = await post<CashToBankOut>(`/shifts/${info.shift_id}/cash-to-bank`, { ...body, op_id: newOpId(), comment, dry_run: true })
      setEffect(out.effect)
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key])

  const submit = () =>
    void save.run(async () => {
      await post<CashToBankOut>(`/shifts/${info.shift_id}/cash-to-bank`, { ...body, op_id: opId, comment: comment.trim() })
      onDone()
    })

  return (
    <Modal title={`Из наличных в QR · смена № ${info.number}`} onClose={onClose}>
      <div className="flex flex-col gap-3 text-sm" data-testid="cash-to-bank">
        <div className="text-slate-600" data-testid="cash-to-bank-limit">
          Наличными по чекам смены {formatSom(info.cash_sales_tyiyn - info.cash_returns_tyiyn)}, перенесено {formatSom(info.moved_tyiyn)}, можно до{' '}
          {formatSom(info.limit_tyiyn)}
          {info.open ? '' : ` · смена закрыта: ${diffWords(info.diff_tyiyn ?? 0, formatSom)}`}
        </div>
        <Field label="Сумма" required>
          <input inputMode="decimal" value={amount} onChange={(e) => setAmount(e.target.value)} data-testid="cash-to-bank-amount" />
        </Field>
        <Field label="Счёт банка" required>
          <select value={account} onChange={(e) => setAccount(e.target.value)}>
            {info.accounts.map((a) => (
              <option key={a.id} value={a.id}>
                {a.name}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Причина" required>
          <input value={comment} maxLength={500} onChange={(e) => setComment(e.target.value)} data-testid="cash-to-bank-comment" />
        </Field>
        {value != null && value > info.limit_tyiyn && <div className="text-rose-700">Больше, чем можно перенести</div>}
        <ErrorBox error={preview.error} />
        <EffectBox effect={effect} fix={fix} onFix={setFix} />
        <ErrorBox error={save.error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={!valid || !comment.trim() || save.busy} onClick={submit} data-testid="cash-to-bank-save">
            Перенести
          </Button>
        </div>
      </div>
    </Modal>
  )
}

/** Переносы смены: когда, кто, сумма, счёт, комиссия, причина, «Отменить». */
export function TransfersList({ items, onChanged }: { items: TransferRow[]; onChanged: () => void }) {
  const owner = useOwnerOnline()
  const [undo, setUndo] = useState<TransferRow | null>(null)
  if (items.length === 0) return null
  return (
    <div className="flex flex-col gap-1 text-sm" data-testid="transfers">
      <div className="font-semibold">Из наличных в QR</div>
      {items.map((t) => (
        <div key={t.id} className={`flex flex-wrap items-center gap-2 ${t.reversed_at ? 'text-slate-400 line-through' : ''}`}>
          <span>
            {formatDateTime(t.created_at)} · {t.user_name} · {formatSom(t.amount_tyiyn)} → {t.account_name}, комиссия {formatSom(t.fee_tyiyn)} ·{' '}
            {t.comment}
          </span>
          {t.reversed_at && <span className="no-underline">отменён: {t.reverse_comment}</span>}
          {!t.reversed_at && owner && (
            <Button variant="secondary" className="min-h-0 px-2 py-1 text-xs" onClick={() => setUndo(t)}>
              Отменить
            </Button>
          )}
        </div>
      ))}
      {undo && (
        <ReverseTransferModal
          t={undo}
          onClose={() => setUndo(null)}
          onDone={() => {
            setUndo(null)
            onChanged()
          }}
        />
      )}
    </div>
  )
}

function ReverseTransferModal({ t, onClose, onDone }: { t: TransferRow; onClose: () => void; onDone: () => void }) {
  const [comment, setComment] = useState('')
  const [opId] = useState(newOpId)
  const act = useAction()
  return (
    <Modal title="Отменить перенос" onClose={onClose}>
      <div className="flex flex-col gap-3 text-sm">
        <div>
          {formatSom(t.amount_tyiyn)} → {t.account_name}, {formatDateTime(t.created_at)}
        </div>
        <Field label="Причина" required>
          <input value={comment} maxLength={500} onChange={(e) => setComment(e.target.value)} />
        </Field>
        <ErrorBox error={act.error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Назад
          </Button>
          <Button
            variant="danger-outline"
            disabled={!comment.trim() || act.busy}
            onClick={() =>
              void act.run(async () => {
                await post(`/cash-to-bank/${t.id}/reverse`, { op_id: opId, comment: comment.trim() })
                onDone()
              })
            }
          >
            Отменить перенос
          </Button>
        </div>
      </div>
    </Modal>
  )
}

/** Блок «После закрытия» отчёта смены (SPEC-37 §4). */
export function AfterCloseBlock({ a }: { a: AfterClose }) {
  const b = a.before
  const n = a.after
  const row = (label: string, x: string, y: string) => (
    <tr className="border-t border-slate-100">
      <td className="py-1 pr-2">{label}</td>
      <td className="py-1 pr-2 text-slate-500">{x}</td>
      <td className="py-1 font-medium">{y}</td>
    </tr>
  )
  return (
    <Notice tone="amber">
      <div className="flex flex-col gap-2 text-sm" data-testid="after-close">
        <div className="font-semibold">{a.at === 'close' ? 'После закрытия' : 'Поправки пересчёта при открытии'}</div>
        {a.items.map((i) => (
          <div key={i.id}>
            {formatDateTime(i.created_at)} · {i.user_name} ·{' '}
            {i.kind === 'sale' && i.original_id && i.new_id ? (
              <>
                <Link className="text-sky-700 hover:underline" to={`/sales/${i.original_id}`}>
                  чек № {i.original_number}
                </Link>{' '}
                →{' '}
                <Link className="text-sky-700 hover:underline" to={`/sales/${i.new_id}`}>
                  № {i.new_number}
                </Link>
              </>
            ) : i.reversal ? (
              'отмена переноса'
            ) : (
              'перенос'
            )}
            : {i.old_text} → {i.new_text}
            {i.withhold_tyiyn !== 0 && `; кассиру ${i.withhold_tyiyn > 0 ? '+' : '−'}${formatSom(Math.abs(i.withhold_tyiyn))}`} · {i.comment}
          </div>
        ))}
        <table className="w-full text-xs">
          <thead>
            <tr className="text-left text-slate-500">
              <th className="py-1 pr-2 font-medium" />
              <th className="py-1 pr-2 font-medium">При закрытии</th>
              <th className="py-1 font-medium">Сейчас</th>
            </tr>
          </thead>
          <tbody>
            {row('Наличными по чекам', formatSom(b.cash_sales_tyiyn), formatSom(n.cash_sales_tyiyn))}
            {n.cash_to_bank_tyiyn !== 0 && row('Из наличных в QR', formatSom(0), formatSom(n.cash_to_bank_tyiyn))}
            {n.bank.map((x) => {
              const o = b.bank.find((y) => y.account_id === x.account_id)
              return (
                <tr key={x.account_id} className="border-t border-slate-100">
                  <td className="py-1 pr-2">{x.name}</td>
                  <td className="py-1 pr-2 text-slate-500">
                    {formatSom(o?.amount_tyiyn ?? 0)}, комиссия {formatSom(o?.fee_tyiyn ?? 0)}
                  </td>
                  <td className="py-1 font-medium">
                    {formatSom(x.amount_tyiyn)}, комиссия {formatSom(x.fee_tyiyn)}
                  </td>
                </tr>
              )
            })}
            {row('Расхождение', diffWords(b.diff_tyiyn, formatSom), diffWords(n.diff_tyiyn, formatSom))}
            {row('Удержано с кассира', formatSom(b.withheld_tyiyn), formatSom(n.withheld_tyiyn))}
            {b.net_profit_tyiyn != null &&
              n.net_profit_tyiyn != null &&
              row('Чистая прибыль', formatSom(b.net_profit_tyiyn), formatSom(n.net_profit_tyiyn))}
          </tbody>
        </table>
      </div>
    </Notice>
  )
}
