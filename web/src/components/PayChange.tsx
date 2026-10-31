/**
 * «Изменить оплату» чека и «Наличные ↔ QR» за смену (SPEC-37, ADR-132, задача 003 Н1). Только при
 * связи с сервером: владелец — всё, администратор с правом «Наличные ↔ QR в открытой смене» — только
 * открытая смена (ADR-{B5.1}). «Что изменится» сервер считает пробным проведением.
 */
import { useEffect, useMemo, useState } from 'react'
import { Link } from 'react-router-dom'
import { get, newOpId, post, qs } from '../lib/api'
import { useAuth } from '../lib/auth'
import { formatDateTime, formatSom, formatTime, parseSom } from '../lib/format'
import { useAction, useDebounced, useLoad, useServerOnline } from '../lib/hooks'
import { useNumField } from '../lib/cashierMode'
import {
  canFixPay,
  diffWords,
  formPayments,
  initialForm,
  matchingQr,
  remaining,
  swapPayments,
  swapTarget,
  transferLimit,
  transferWords,
  type AfterClose,
  type CashToBankInfo,
  type CashToBankOut,
  type Direction,
  type Effect,
  type PartyBrief,
  type PayChangeInfo,
  type PayChangeOut,
  type PayForm,
  type SalePayBrief,
  type ShiftSaleFix,
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

/** Исправлять оплату смены: владелец при связи; администратор с правом — в открытой смене. */
export function useCanFixPay(shiftOpen: boolean): boolean {
  const { user } = useAuth()
  const online = useServerOnline()
  return canFixPay(user, online, shiftOpen)
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
  // Администратору (ADR-{B5.1}) — только «Наличные» и «Карта / QR банка» без привязки QR.
  const owner = useAuth().user?.role === 'owner'
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
          {owner && qr.length > 0 && (
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
          {owner && (
            <div className="flex flex-wrap items-center gap-2">
              <span className="w-36">{PAYMENT_LABELS.transfer}</span>
              {sumInput('transfer')}
              {info.accounts.length > 0 && accountSelect('transferAccount')}
            </div>
          )}
          {owner && company && (
            <div className="flex flex-wrap items-center gap-2">
              <span className="w-36">{PAYMENT_LABELS.invoice}</span>
              {sumInput('invoice')}
              {info.accounts.length > 0 && accountSelect('invoiceAccount')}
            </div>
          )}
          {owner && (
            <div className="flex flex-wrap items-center gap-2">
              <span className="w-36">{PAYMENT_LABELS.debt}</span>
              {sumInput('debt')}
            </div>
          )}
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

/** Кнопка «Наличные ↔ QR»: владельцу — всегда при связи, администратору с правом — в открытой смене. */
export function CashToBankButton({ onClick, open, className }: { onClick: () => void; open: boolean; className?: string }) {
  if (!useCanFixPay(open)) return null
  return (
    <Button variant="secondary" className={className} onClick={onClick} data-testid="cash-to-bank-button">
      Наличные ↔ QR
    </Button>
  )
}

/**
 * Окно «Наличные ↔ QR» за смену (SPEC-37 §3, задача 003 Н1): «Одной суммой» в обе стороны и «По чекам».
 * `onDone` — перенос одной суммой проведён (окно закрывают); `onChanged` — поменяли оплату чека, окно
 * остаётся открытым.
 */
export function CashToBankModal({
  shiftId,
  onClose,
  onDone,
  onChanged,
}: {
  shiftId: string
  onClose: () => void
  onDone: () => void
  onChanged?: () => void
}) {
  const info = useLoad(() => get<CashToBankInfo>(`/shifts/${shiftId}/cash-to-bank`), [shiftId])
  const [tab, setTab] = useState<'sum' | 'sales'>('sum')
  const d = info.data
  if (!d) {
    return (
      <Modal title="Наличные ↔ QR" onClose={onClose}>
        {info.error ? <ErrorBox error={info.error} /> : <Loading />}
      </Modal>
    )
  }
  const tabButton = (key: 'sum' | 'sales', label: string) => (
    <button
      type="button"
      role="tab"
      aria-selected={tab === key}
      className={`min-h-10 rounded-md border px-3 py-1 text-sm ${tab === key ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white hover:border-sky-500'}`}
      onClick={() => setTab(key)}
      data-testid={`cash-to-bank-tab-${key}`}
    >
      {label}
    </button>
  )
  return (
    <Modal title={`Наличные ↔ QR · смена № ${d.number}`} onClose={onClose} wide>
      <div className="mb-3 flex flex-wrap gap-2" role="tablist">
        {tabButton('sum', 'Одной суммой')}
        {tabButton('sales', 'По чекам')}
      </div>
      {tab === 'sum' ? (
        <CashToBankForm info={d} onClose={onClose} onDone={onDone} />
      ) : (
        <ShiftSalesPayFix
          info={d}
          onChanged={() => {
            info.reload()
            onChanged?.()
          }}
        />
      )}
    </Modal>
  )
}

function CashToBankForm({ info, onClose, onDone }: { info: CashToBankInfo; onClose: () => void; onDone: () => void }) {
  const { user } = useAuth()
  const owner = user?.role === 'owner'
  const num = useNumField()
  const [direction, setDirection] = useState<Direction>('to_bank')
  const [amount, setAmount] = useState('')
  const [account, setAccount] = useState((info.accounts.find((a) => a.default_card) ?? info.accounts[0])?.id ?? '')
  const [comment, setComment] = useState('')
  const [fix, setFix] = useState(true)
  const [opId, setOpId] = useState(newOpId)
  const [effect, setEffect] = useState<Effect | null>(null)
  const preview = useAction()
  const save = useAction()
  const value = parseSom(amount.trim())
  const limit = transferLimit(info, direction, account)
  // Предел владельца на администратора за смену (настройка «Смена»).
  const adminLeft = !owner && info.admin_max_tyiyn != null ? Math.max(0, info.admin_max_tyiyn - info.admin_used_tyiyn) : null
  const max = adminLeft == null ? limit : Math.min(limit, adminLeft)
  const valid = value != null && value > 0 && value <= max && account !== ''
  const key = useDebounced(valid ? `${direction}|${value}|${account}|${fix}` : '', 300)
  const body = { direction, amount_tyiyn: value ?? 0, account_id: account, fix_withholding: fix }
  const bank = info.bank_limits.find((b) => b.account_id === account)

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

  const pick = (d: Direction) => {
    setDirection(d)
    // Другое направление — другая операция: свой op_id (I8).
    setOpId(newOpId())
  }

  return (
    <div className="flex flex-col gap-3 text-sm" data-testid="cash-to-bank">
      <div className="flex flex-wrap gap-2" data-testid="cash-to-bank-direction">
        {(
          [
            ['to_bank', 'Из наличных в QR'],
            ['to_cash', 'Из QR в наличные'],
          ] as const
        ).map(([d, label]) => (
          <button
            key={d}
            type="button"
            aria-pressed={direction === d}
            className={`min-h-10 rounded-md border px-3 py-1 ${direction === d ? 'border-sky-600 bg-sky-50 font-medium text-sky-800' : 'border-slate-300 bg-white hover:border-sky-500'}`}
            onClick={() => pick(d)}
            data-testid={`cash-to-bank-${d}`}
          >
            {label}
          </button>
        ))}
      </div>
      <div className="text-slate-600" data-testid="cash-to-bank-limit">
        {direction === 'to_bank' ? (
          <>
            Наличными по чекам смены {formatSom(info.cash_sales_tyiyn - info.cash_returns_tyiyn)}, перенесено {formatSom(info.moved_tyiyn)}, можно
            до {formatSom(limit)}
          </>
        ) : (
          <>
            «Карта / QR банка» по чекам смены на этот счёт без привязанного QR {formatSom((bank?.card_sales_tyiyn ?? 0) - (bank?.card_returns_tyiyn ?? 0))},
            перенесено на счёт {formatSom(bank?.moved_tyiyn ?? 0)}, можно до {formatSom(limit)}
          </>
        )}
        {info.open ? '' : ` · смена закрыта: ${diffWords(info.diff_tyiyn ?? 0, formatSom)}`}
        {adminLeft != null && ` · вам владелец разрешил ещё ${formatSom(adminLeft)} за смену`}
      </div>
      <Field label="Сумма" required>
        <input {...num('decimal')} value={amount} onChange={(e) => setAmount(e.target.value)} data-testid="cash-to-bank-amount" />
      </Field>
      <Field label="Счёт банка" required>
        <select value={account} onChange={(e) => setAccount(e.target.value)} data-testid="cash-to-bank-account">
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
      {value != null && value > max && <div className="text-rose-700">Больше, чем можно перенести</div>}
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
  )
}

/** Оплата чека в строке: «наличные 1 000 с», «карта / QR банка (Optima) 500 с». */
function payText(payments: SalePayBrief[]): string {
  return payments.map((p) => `${PAYMENT_LABELS[p.method]}${p.account_name && p.method !== 'cash' ? ` (${p.account_name})` : ''} ${formatSom(p.amount_tyiyn)}`).join(', ')
}

/**
 * Вкладка «По чекам»: чеки смены с наличными или «Картой / QR». «⇄ в QR» / «⇄ в наличные» — это
 * «Изменить оплату» чека всей суммой способа (SPEC-37 §2); смешанная оплата — «Подробно».
 */
function ShiftSalesPayFix({ info, onChanged }: { info: CashToBankInfo; onChanged: () => void }) {
  const [filter, setFilter] = useState<'all' | 'cash' | 'card'>('all')
  const [swap, setSwap] = useState<{ sale: ShiftSaleFix; to: 'card' | 'cash' } | null>(null)
  const [full, setFull] = useState<string | null>(null)
  const shown = info.sales.filter((s) => filter === 'all' || s.payments.some((p) => p.method === filter))
  return (
    <div className="flex flex-col gap-2 text-sm" data-testid="shift-sales-pay-fix">
      <div className="flex flex-wrap gap-2">
        {(
          [
            ['all', 'Все'],
            ['cash', 'Наличные'],
            ['card', 'Карта / QR'],
          ] as const
        ).map(([k, label]) => (
          <button
            key={k}
            type="button"
            aria-pressed={filter === k}
            className={`min-h-9 rounded-md border px-3 py-1 ${filter === k ? 'border-sky-600 bg-sky-50 text-sky-800' : 'border-slate-300 bg-white'}`}
            onClick={() => setFilter(k)}
          >
            {label}
          </button>
        ))}
      </div>
      {shown.length === 0 && <div className="text-slate-500">Чеков нет</div>}
      {shown.map((s) => {
        const to = swapTarget(s.payments)
        return (
          <div key={s.id} className="flex flex-wrap items-center gap-2 border-t border-slate-100 py-1" data-testid="shift-sale-row">
            <span className="w-16 font-medium">№ {s.number}</span>
            <span className="w-14 text-slate-500">{formatTime(s.created_at)}</span>
            <span className="w-24 text-right tabular-nums">{formatSom(s.total_tyiyn)}</span>
            <span className="min-w-0 flex-1 text-slate-600">{payText(s.payments)}</span>
            {s.fixable ? (
              <>
                {to && (
                  <Button variant="secondary" className="min-h-0 px-2 py-1 text-xs" onClick={() => setSwap({ sale: s, to })} data-testid="shift-sale-swap">
                    {to === 'card' ? '⇄ в QR' : '⇄ в наличные'}
                  </Button>
                )}
                <Button variant="secondary" className="min-h-0 px-2 py-1 text-xs" onClick={() => setFull(s.id)}>
                  Подробно
                </Button>
              </>
            ) : (
              <span className="text-xs text-slate-400">{s.reason}</span>
            )}
          </div>
        )
      })}
      {swap && (
        <SwapModal
          sale={swap.sale}
          to={swap.to}
          accounts={info.accounts}
          onClose={() => setSwap(null)}
          onDone={() => {
            setSwap(null)
            onChanged()
          }}
        />
      )}
      {full && (
        <PayChangeModal
          saleId={full}
          onClose={() => setFull(null)}
          onSaved={() => {
            setFull(null)
            onChanged()
          }}
        />
      )}
    </div>
  )
}

/** «⇄ в QR» / «⇄ в наличные» чека: «Что изменится», причина и «Сохранить» — одно «Изменить оплату». */
function SwapModal({
  sale,
  to,
  accounts,
  onClose,
  onDone,
}: {
  sale: ShiftSaleFix
  to: 'card' | 'cash'
  accounts: CashToBankInfo['accounts']
  onClose: () => void
  onDone: () => void
}) {
  const [account, setAccount] = useState(
    sale.payments.find((p) => p.method === 'card')?.account_id ?? (accounts.find((a) => a.default_card) ?? accounts[0])?.id ?? '',
  )
  const [comment, setComment] = useState('')
  const [fix, setFix] = useState(true)
  const [opId] = useState(newOpId)
  const [effect, setEffect] = useState<Effect | null>(null)
  const preview = useAction()
  const save = useAction()
  const payments = swapPayments(sale.payments, to, account)
  const key = useDebounced(`${JSON.stringify(payments)}|${fix}`, 200)

  useEffect(() => {
    void preview.run(async () => {
      const out = await post<PayChangeOut>(`/sales/${sale.id}/pay-change`, { payments, fix_withholding: fix, op_id: newOpId(), comment, dry_run: true })
      setEffect(out.effect)
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key])

  const submit = () =>
    void save.run(async () => {
      await post<PayChangeOut>(`/sales/${sale.id}/pay-change`, { payments, fix_withholding: fix, op_id: opId, comment: comment.trim() })
      onDone()
    })

  return (
    <Modal title={`Чек № ${sale.number}: ${to === 'card' ? 'в QR' : 'в наличные'}`} onClose={onClose}>
      <div className="flex flex-col gap-3 text-sm" data-testid="shift-sale-swap-modal">
        <div className="text-slate-600">
          {payText(sale.payments)} → {to === 'card' ? PAYMENT_LABELS.card : PAYMENT_LABELS.cash} {formatSom(payments[0]?.amount_tyiyn ?? 0)}
        </div>
        {to === 'card' && accounts.length > 1 && (
          <Field label="Счёт банка" required>
            <select value={account} onChange={(e) => setAccount(e.target.value)}>
              {accounts.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.name}
                </option>
              ))}
            </select>
          </Field>
        )}
        <Field label="Что и почему меняете" required>
          <input value={comment} maxLength={500} onChange={(e) => setComment(e.target.value)} data-testid="shift-sale-swap-comment" autoFocus />
        </Field>
        <ErrorBox error={preview.error} />
        <EffectBox effect={effect} fix={fix} onFix={setFix} />
        <ErrorBox error={save.error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={!comment.trim() || save.busy || (to === 'card' && !account)} onClick={submit} data-testid="shift-sale-swap-save">
            Сохранить
          </Button>
        </div>
      </div>
    </Modal>
  )
}

/**
 * Переносы смены: когда, кто, куда, сумма, комиссия, причина, «Отменить». Владелец отменяет любой;
 * администратор с правом — только свой, пока смена открыта (ADR-{B5.1}).
 */
export function TransfersList({ items, open, onChanged }: { items: TransferRow[]; open: boolean; onChanged: () => void }) {
  const { user } = useAuth()
  const can = useCanFixPay(open)
  const [undo, setUndo] = useState<TransferRow | null>(null)
  if (items.length === 0) return null
  return (
    <div className="flex flex-col gap-1 text-sm" data-testid="transfers">
      <div className="font-semibold">Наличные ↔ QR</div>
      {items.map((t) => (
        <div key={t.id} className={`flex flex-wrap items-center gap-2 ${t.reversed_at ? 'text-slate-400 line-through' : ''}`} data-testid="transfer-row">
          <span>
            {formatDateTime(t.created_at)} · {t.user_name} · {formatSom(Math.abs(t.amount_tyiyn))} {transferWords(t.amount_tyiyn, t.account_name)}, комиссия{' '}
            {t.amount_tyiyn < 0 ? 'возвращена ' : ''}
            {formatSom(Math.abs(t.fee_tyiyn))} · {t.comment}
          </span>
          {t.reversed_at && <span className="no-underline">отменён: {t.reverse_comment}</span>}
          {!t.reversed_at && can && (user?.role === 'owner' || (t.user_id === user?.id && !t.counted)) && (
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
          {formatSom(Math.abs(t.amount_tyiyn))} {transferWords(t.amount_tyiyn, t.account_name)}, {formatDateTime(t.created_at)}
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
              'перенос наличные ↔ QR'
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
            {n.cash_to_bank_tyiyn !== 0 &&
              row(n.cash_to_bank_tyiyn > 0 ? 'Из наличных в QR' : 'Из QR в наличные', formatSom(0), formatSom(Math.abs(n.cash_to_bank_tyiyn)))}
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
