// Брак (SPEC-25): отметка из остатков, вкладка «Брак» с решением владельца и сторно.
import { useState } from 'react'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, Modal, Notice, Table, toast } from './ui'
import { get, newOpId, post, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { RESOLUTION_LABELS, STATUS_LABELS, parseQty, qtyInput, qtyInputLabel, qtyText, refundEffect, type ResolutionKind } from '../lib/defects'
import { formatDateTime, formatSom, parseSom, somInput } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import type { CashAccount, Defect, Product, Supplier } from '../lib/types'

/** «В брак»: количество в единицах товара, причина и поставщик (по умолчанию — из последнего прихода). */
export function DefectMarkModal({ product, onClose, onDone }: { product: Product; onClose: () => void; onDone: () => void }) {
  const [qty, setQty] = useState('')
  const [reason, setReason] = useState('')
  const [supplier, setSupplier] = useState<string | null>(null)
  const [opId] = useState(newOpId)
  const act = useAction()
  const suppliers = useLoad(() => get<Supplier[]>('/suppliers'), [])
  const hint = useLoad(
    () => get<{ supplier_id: string | null; supplier_name: string | null }>(`/defects/supplier-hint${qs({ product_id: product.id })}`),
    [product.id],
  )
  // Пока не выбрали сами — поставщик последнего прихода.
  const chosen = supplier ?? hint.data?.supplier_id ?? ''
  const parsed = parseQty(qty, product.unit)
  const tooMuch = parsed !== null && parsed > product.stock_qty
  const save = () => {
    if (parsed === null) return act.setError('Укажите количество')
    if (!reason.trim()) return act.setError('Укажите причину')
    void act.run(async () => {
      await post('/defects', {
        op_id: opId,
        product_id: product.id,
        qty: parsed,
        reason: reason.trim(),
        supplier_id: chosen || undefined,
        no_supplier: !chosen,
      })
      toast(`В брак: ${product.name} × ${qtyText(parsed, product.unit)}`)
      onDone()
    })
  }
  return (
    <Modal title={`В брак · ${product.name}`} onClose={onClose}>
      <div className="flex flex-col gap-3">
        <p className="text-sm text-slate-600">
          Товар уйдёт из остатка и не будет продаваться. Решение — списать или вернуть поставщику — принимает владелец. На складе{' '}
          {qtyText(product.stock_qty, product.unit)}.
        </p>
        <Field label={`Количество, ${qtyInputLabel(product.unit)}`} required error={tooMuch ? 'Больше, чем на складе' : null}>
          <input autoFocus inputMode="decimal" name="defect-qty" value={qty} onChange={(e) => setQty(e.target.value)} />
        </Field>
        <Field label="Причина" required>
          <input name="defect-reason" placeholder="течёт канистра, битый корпус, не тот размер" value={reason} onChange={(e) => setReason(e.target.value)} />
        </Field>
        <Field label="Поставщик" hint={hint.data?.supplier_name ? `по последнему приходу — ${hint.data.supplier_name}` : undefined}>
          <select name="defect-supplier" value={chosen} onChange={(e) => setSupplier(e.target.value)}>
            <option value="">— не указан —</option>
            {(suppliers.data ?? [])
              .filter((s) => s.active || s.id === chosen)
              .map((s) => (
                <option key={s.id} value={s.id}>
                  {s.name}
                </option>
              ))}
          </select>
        </Field>
        <ErrorBox error={act.error ?? suppliers.error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={act.busy || parsed === null || tooMuch || !reason.trim()} onClick={save}>
            В брак
          </Button>
        </div>
      </div>
    </Modal>
  )
}

/** Решение владельца: четыре варианта, сумма возмещения по умолчанию = стоимость брака. */
function ResolveModal({ defect, onClose, onDone }: { defect: Defect; onClose: () => void; onDone: () => void }) {
  const cost = defect.cost_tyiyn ?? 0
  const [kind, setKind] = useState<ResolutionKind>('write_off')
  const [replace, setReplace] = useState(qtyInput(defect.qty, defect.unit))
  const [refund, setRefund] = useState(somInput(cost))
  const [account, setAccount] = useState('')
  const [supplier, setSupplier] = useState(defect.supplier_id ?? '')
  const [comment, setComment] = useState('')
  const [opId] = useState(newOpId)
  const act = useAction()
  const suppliers = useLoad(() => get<Supplier[]>('/suppliers'), [])
  const accounts = useLoad(() => get<CashAccount[]>('/cash/accounts'), [])
  const refundValue = parseSom(refund)
  const replaceQty = parseQty(replace, defect.unit)
  const missing =
    kind !== 'write_off' && !supplier
      ? 'Выберите поставщика'
      : kind === 'replace' && (replaceQty === null || replaceQty > defect.qty)
        ? 'Количество взамен — не больше брака'
        : (kind === 'credit' || kind === 'cash') && (refundValue === null || refundValue <= 0)
          ? 'Укажите сумму возмещения'
          : kind === 'cash' && !account
            ? 'Выберите кассу'
            : null
  const save = () => {
    if (missing) return act.setError(missing)
    void act.run(async () => {
      await post(`/defects/${defect.id}/resolve`, {
        op_id: opId,
        kind,
        supplier_id: kind === 'write_off' ? undefined : supplier,
        replace_qty: kind === 'replace' ? replaceQty : undefined,
        refund_tyiyn: kind === 'credit' || kind === 'cash' ? refundValue : undefined,
        account_id: kind === 'cash' ? account : undefined,
        comment: comment.trim(),
      })
      toast(`Брак № ${defect.number}: ${RESOLUTION_LABELS[kind].toLowerCase()}`)
      onDone()
    })
  }
  return (
    <Modal title={`Решение по браку № ${defect.number}`} onClose={onClose}>
      <div className="flex flex-col gap-3">
        <div className="text-sm">
          <div className="font-medium">
            {defect.product_name} × {qtyText(defect.qty, defect.unit)}
          </div>
          <div className="text-slate-600">
            Стоимость {formatSom(cost)} · {defect.reason}
          </div>
        </div>
        <div className="grid grid-cols-1 gap-2 sm:grid-cols-2" role="radiogroup" aria-label="Решение">
          {(Object.keys(RESOLUTION_LABELS) as ResolutionKind[]).map((k) => (
            <button
              key={k}
              type="button"
              role="radio"
              aria-checked={kind === k}
              className={`min-h-[42px] rounded-md border px-3 py-2 text-left text-sm ${kind === k ? 'border-sky-600 bg-sky-50 font-semibold' : 'border-slate-300 hover:bg-slate-50'}`}
              onClick={() => setKind(k)}
            >
              {RESOLUTION_LABELS[k]}
            </button>
          ))}
        </div>
        {kind === 'write_off' && <Notice tone="amber">Убыток {formatSom(cost)} — строка «Списание брака» в отчёте о прибыли.</Notice>}
        {kind !== 'write_off' && (
          <Field label="Поставщик" required>
            <select value={supplier} onChange={(e) => setSupplier(e.target.value)}>
              <option value="">— выберите —</option>
              {(suppliers.data ?? []).map((s) => (
                <option key={s.id} value={s.id}>
                  {s.name}
                </option>
              ))}
            </select>
          </Field>
        )}
        {kind === 'replace' && (
          <Field label={`Привёз взамен, ${qtyInputLabel(defect.unit)}`} hint="Если меньше брака — недостающее спишется в убыток">
            <input inputMode="decimal" value={replace} onChange={(e) => setReplace(e.target.value)} />
          </Field>
        )}
        {(kind === 'credit' || kind === 'cash') && (
          <Field label="Сумма возмещения, с" required hint={refundValue !== null ? `${refundEffect(cost, refundValue)} — строка «Брак: разница возмещения»` : undefined}>
            <input inputMode="decimal" name="defect-refund" value={refund} onChange={(e) => setRefund(e.target.value)} />
          </Field>
        )}
        {kind === 'credit' && (
          <p className="text-sm text-slate-600">Поставщик будет должен нам эту сумму; следующая накладная в долг зачтёт её сама.</p>
        )}
        {kind === 'cash' && (
          <Field label="Куда пришли деньги" required>
            <select value={account} onChange={(e) => setAccount(e.target.value)}>
              <option value="">— касса —</option>
              {(accounts.data ?? []).map((a) => (
                <option key={a.id} value={a.id}>
                  {a.name}
                </option>
              ))}
            </select>
          </Field>
        )}
        <Field label="Комментарий">
          <input value={comment} onChange={(e) => setComment(e.target.value)} />
        </Field>
        <ErrorBox error={act.error ?? suppliers.error ?? accounts.error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={act.busy} onClick={save}>
            Провести
          </Button>
        </div>
      </div>
    </Modal>
  )
}

function resolutionText(d: Defect): string {
  const r = d.resolution
  if (!r) return STATUS_LABELS[d.status] ?? d.status
  const parts = [RESOLUTION_LABELS[r.kind]]
  if (r.replace_qty !== null) parts.push(`взамен ${qtyText(r.replace_qty, d.unit)}`)
  if (r.refund_tyiyn !== null) parts.push(`возмещение ${formatSom(r.refund_tyiyn)}`)
  if (r.account_name) parts.push(r.account_name)
  if (r.writeoff_tyiyn) parts.push(`в убыток ${formatSom(r.writeoff_tyiyn)}`)
  if (r.diff_tyiyn) parts.push(r.diff_tyiyn > 0 ? `доход ${formatSom(r.diff_tyiyn)}` : `убыток ${formatSom(-r.diff_tyiyn)}`)
  return parts.join(' · ')
}

/** Вкладка «Брак»: ожидают решения и решённые. Решает и отменяет решение только владелец. */
export function DefectsTab() {
  const owner = useUser().role === 'owner'
  const [view, setView] = useState<'pending' | 'resolved'>('pending')
  const list = useLoad(() => get<Defect[]>(`/defects${qs({ status: view })}`), [view])
  const [resolving, setResolving] = useState<Defect | null>(null)
  const act = useAction()
  const unmark = (d: Defect) => {
    if (!window.confirm(`Снять отметку брака № ${d.number}? Товар вернётся в остаток.`)) return
    void act.run(async () => {
      await post(`/defects/${d.id}/unmark`, { op_id: newOpId() })
      toast('Отметка снята, товар вернулся в остаток')
      list.reload()
    })
  }
  const reverse = (d: Defect) => {
    const comment = window.prompt('Причина отмены решения')
    if (!comment?.trim()) return
    void act.run(async () => {
      await post(`/defects/${d.id}/resolution/reverse`, { op_id: newOpId(), comment: comment.trim() })
      toast('Решение отменено — брак снова ждёт решения')
      list.reload()
    })
  }
  const rows = list.data ?? []
  const head = ['№', 'Товар', 'Количество', ...(owner ? ['Стоимость'] : []), 'Причина', 'Поставщик', 'Отметил', view === 'pending' ? '' : 'Решение', '']
  return (
    <Card>
      <div className="mb-3 flex gap-2 text-sm">
        {(
          [
            ['pending', 'Ожидают решения'],
            ['resolved', 'Решённые'],
          ] as const
        ).map(([key, label]) => (
          <button
            key={key}
            type="button"
            aria-pressed={view === key}
            className={`rounded-full border px-3 py-1.5 ${view === key ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white hover:bg-slate-50'}`}
            onClick={() => setView(key)}
          >
            {label}
          </button>
        ))}
      </div>
      <ErrorBox error={list.error ?? act.error} />
      {list.loading && !list.data ? (
        <Loading />
      ) : rows.length === 0 ? (
        <Empty icon="stock">{view === 'pending' ? 'Брака, ждущего решения, нет. Отметить брак — кнопка «В брак» в строке товара.' : 'Решённого брака пока нет'}</Empty>
      ) : (
        <Table head={head}>
          {rows.map((d) => (
            <tr key={d.id} className="hover:bg-slate-50">
              <td className="px-2 py-2 font-medium">№ {d.number}</td>
              <td className="px-2 py-2">
                <div className="font-medium">{d.product_name}</div>
                <div className="text-xs text-slate-500">{formatDateTime(d.created_at)}</div>
              </td>
              <td className="whitespace-nowrap px-2 py-2">{qtyText(d.qty, d.unit)}</td>
              {owner && <td className="whitespace-nowrap px-2 py-2">{formatSom(d.cost_tyiyn ?? 0)}</td>}
              <td className="px-2 py-2">{d.reason}</td>
              <td className="px-2 py-2">{d.resolution?.supplier_name ?? d.supplier_name ?? '—'}</td>
              <td className="px-2 py-2">{d.user_name}</td>
              <td className="px-2 py-2">
                {view === 'pending' ? (
                  <Badge tone="amber">ждёт решения</Badge>
                ) : (
                  <span className="text-sm">
                    {d.status === 'cancelled' ? <Badge tone="slate">отметка снята</Badge> : resolutionText(d)}
                    {d.resolution?.comment && <span className="block text-xs text-slate-500">{d.resolution.comment}</span>}
                  </span>
                )}
              </td>
              <td className="whitespace-nowrap px-2 py-2 text-right">
                {d.status === 'pending' && (
                  <div className="flex justify-end gap-2">
                    <Button variant="secondary" className="min-h-[36px] py-1 text-xs" disabled={act.busy} onClick={() => unmark(d)}>
                      Снять отметку
                    </Button>
                    {owner && (
                      <Button className="min-h-[36px] py-1 text-xs" onClick={() => setResolving(d)}>
                        Решить
                      </Button>
                    )}
                  </div>
                )}
                {owner && (d.status === 'written_off' || d.status === 'returned') && (
                  <Button variant="secondary" className="min-h-[36px] py-1 text-xs" disabled={act.busy} onClick={() => reverse(d)}>
                    Отменить решение
                  </Button>
                )}
              </td>
            </tr>
          ))}
        </Table>
      )}
      {resolving && (
        <ResolveModal
          defect={resolving}
          onClose={() => setResolving(null)}
          onDone={() => {
            setResolving(null)
            list.reload()
          }}
        />
      )}
    </Card>
  )
}
