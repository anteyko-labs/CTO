// Приёмка в «Приходе» (SPEC-39 §2, §4): блок «Ждут приёмки» с суммами — «Открыть» и «Убрать с
// приёмки»; в накладной — кто принял, расхождения и подпись кассира.
import { useState } from 'react'
import { Link } from 'react-router-dom'
import { reasonLabel, qtyWords, type Acceptance, type AcceptanceList, type AcceptanceLine } from '../lib/acceptances'
import { get, newOpId, post } from '../lib/api'
import { formatDateTime, formatSom } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { Badge, Button, Card, CardTitle, ErrorBox, Field, Loading, Modal, Table, toast } from './ui'

/** Блок «Ждут приёмки: N». Пустой список — блока нет. */
export function AcceptancesWaitingBlock({ list, onChanged }: { list: AcceptanceList | null; onChanged: () => void }) {
  const [open, setOpen] = useState<string | null>(null)
  const [cancelling, setCancelling] = useState<string | null>(null)
  const items = list?.items ?? []
  if (items.length === 0) return null
  return (
    <Card data-testid="acceptances-waiting">
      <CardTitle>Ждут приёмки: {items.length}</CardTitle>
      <ul className="divide-y divide-slate-100">
        {items.map((a) => (
          <li key={a.id} className="flex flex-wrap items-center gap-3 py-2 text-sm" data-testid="acceptance-waiting-row">
            <div className="min-w-0 flex-1">
              <div className="font-medium">
                {a.supplier_name}
                {a.supplier_doc && `, накладная № ${a.supplier_doc}`}
              </div>
              <div className="text-xs text-slate-500">
                {a.lines_count} строк{a.total_tyiyn !== null && ` · ${formatSom(a.total_tyiyn)}`} · {a.sent_by_name} · {formatDateTime(a.sent_at)}
                {a.from_bot && ' · из бота'}
              </div>
            </div>
            <Button variant="secondary" onClick={() => setOpen(a.id)}>
              Открыть
            </Button>
            <Button variant="secondary" data-testid="acceptance-cancel" onClick={() => setCancelling(a.id)}>
              Убрать с приёмки
            </Button>
          </li>
        ))}
      </ul>
      {open && <AcceptanceModal id={open} onClose={() => setOpen(null)} />}
      {cancelling && (
        <CancelModal
          id={cancelling}
          onClose={() => setCancelling(null)}
          onDone={() => {
            setCancelling(null)
            onChanged()
          }}
        />
      )}
    </Card>
  )
}

function CancelModal({ id, onClose, onDone }: { id: string; onClose: () => void; onDone: () => void }) {
  const act = useAction()
  const [opId] = useState(newOpId)
  const [reason, setReason] = useState('')
  return (
    <Modal title="Убрать с приёмки" onClose={onClose}>
      <div className="flex flex-col gap-3">
        <div className="text-sm text-slate-600">Накладная по фото снова станет «новой»; изменить приёмку — убрать и отправить заново.</div>
        <Field label="Причина">
          <input value={reason} maxLength={500} autoFocus data-testid="acceptance-cancel-reason" onChange={(e) => setReason(e.target.value)} />
        </Field>
        <ErrorBox error={act.error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button
            variant="danger"
            disabled={act.busy || !reason.trim()}
            data-testid="acceptance-cancel-confirm"
            onClick={() =>
              void act.run(async () => {
                await post(`/acceptances/${id}/cancel`, { op_id: opId, reason: reason.trim() })
                toast('Убрано с приёмки')
                onDone()
              })
            }
          >
            Убрать
          </Button>
        </div>
      </div>
    </Modal>
  )
}

function AcceptanceModal({ id, onClose }: { id: string; onClose: () => void }) {
  const a = useLoad(() => get<Acceptance>(`/acceptances/${id}`), [id])
  return (
    <Modal title="Приёмка" wide onClose={onClose}>
      <ErrorBox error={a.error} />
      {a.data ? <AcceptanceDetails a={a.data} /> : !a.error && <Loading />}
    </Modal>
  )
}

const accepted = (l: AcceptanceLine) => (l.accepted_qty === null ? '—' : qtyWords(l, l.accepted_qty))

/** Строки приёмки: по накладной, принято, замена, причина; суммы — если видны. */
export function AcceptanceDetails({ a }: { a: Acceptance }) {
  const closed = a.status !== 'waiting'
  return (
    <div className="flex flex-col gap-3 text-sm">
      <div>
        {a.supplier_name}
        {a.supplier_doc && `, накладная № ${a.supplier_doc}`} · отправил {a.sent_by_name} {formatDateTime(a.sent_at)}
        {a.total_tyiyn !== null && ` · по накладной ${formatSom(a.total_tyiyn)}`}
      </div>
      {a.comment && <div className="text-slate-600">Комментарий: {a.comment}</div>}
      <Table head={['Товар', 'По накладной', ...(closed ? ['Принято'] : []), ...(a.total_tyiyn !== null ? ['Сумма строки'] : [])]}>
        {a.lines.map((l) => (
          <tr key={l.line_no}>
            <td className="px-2 py-1.5">
              {l.name}
              {l.accepted_name && <div className="text-amber-800">вместо него принят: {l.accepted_name}</div>}
              {l.reason && <div className="text-amber-800">{reasonLabel(l.reason)}</div>}
              {l.comment && <div className="text-slate-500">«{l.comment}»</div>}
            </td>
            <td className="whitespace-nowrap px-2 py-1.5">{qtyWords(l, l.invoice_qty)}</td>
            {closed && (
              <td className={`whitespace-nowrap px-2 py-1.5 ${l.accepted_qty !== l.invoice_qty ? 'font-semibold text-amber-800' : ''}`}>{accepted(l)}</td>
            )}
            {a.total_tyiyn !== null && <td className="whitespace-nowrap px-2 py-1.5 text-right">{formatSom(l.line_cost_tyiyn ?? 0)}</td>}
          </tr>
        ))}
      </Table>
      {a.extras.length > 0 && (
        <div>
          Не по накладной (в приход не вошло): {a.extras.map((x) => `${x.name || x.barcode} — ${x.qty} шт`).join('; ')}
        </div>
      )}
    </div>
  )
}

/** В накладной: «Принято на планшете» — кассир, расхождения, подпись (SPEC-39 §4). */
export function ReceiptAcceptance({ receiptId }: { receiptId: string }) {
  const a = useLoad(() => get<Acceptance | null>(`/receipts/${receiptId}/acceptance`).catch(() => null), [receiptId])
  if (!a.data) return null
  const x = a.data
  return (
    <Card data-testid="receipt-acceptance">
      <CardTitle>
        Принято на планшете: {x.employee_name ?? '—'}
        {x.closed_at && <span className="ml-2 text-sm font-normal text-slate-500">{formatDateTime(x.closed_at)}</span>}
        {x.receipt_state === 'reversed' && (
          <span className="ml-2">
            <Badge tone="rose">приход сторнирован</Badge>
          </span>
        )}
        {x.receipt_state === 'corrected' && x.corrected_id && (
          <span className="ml-2">
            <Badge tone="amber">
              исправлен → <Link to={`/receipts/${x.corrected_id}`}>№ {x.corrected_number}</Link>
            </Badge>
          </span>
        )}
      </CardTitle>
      <div className="flex flex-col gap-3 md:flex-row">
        <div className="min-w-0 flex-1">
          <AcceptanceDetails a={x} />
          {x.accept_comment && <div className="mt-2 text-sm text-slate-600">Комментарий кассира: {x.accept_comment}</div>}
        </div>
        {x.has_signature && (
          <img src={`/api/v1/acceptances/${x.id}/signature`} alt="подпись кассира" className="h-28 w-auto self-start rounded border border-slate-300 bg-white" data-testid="receipt-acceptance-signature" />
        )}
      </div>
    </Card>
  )
}
