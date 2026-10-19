// Оприходование без поставщика (SPEC-29 §8): количество и цена закупки, комментарий обязателен;
// без накладной и без денег. Ниже — прошлые оприходования товара со сторно и исправлением.
import { useState } from 'react'
import { get, newOpId, post, qs } from '../lib/api'
import { formatDateTime, formatKg, formatLiters, formatSom, parseSom, somInput } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import { stockInCost, stockInQty } from '../lib/stockIn'
import { correctionMarks, loadCorrections } from '../lib/corrections'
import { amountInput, fixedText, qtyInput } from '../lib/docFix'
import type { Product, StockIn } from '../lib/types'
import { CorrectionBadges, FixButton } from './Correction'
import { FIX_SUBMIT, FixBanner, FixDraft, type FixMode } from './FixDraft'
import { Badge, Button, ErrorBox, Field, Missing, Modal, toast } from './ui'

const qtyText = (unit: string, q: number) => (unit === 'ml' ? formatLiters(q) : unit === 'g' ? formatKg(q) : `${q} шт`)

/** Оприходование в виде запроса проведения (без op_id). */
interface StockInDoc {
  product_id: string
  qty: number
  price_tyiyn: number
  comment: string
}

/** Форма оприходования; с `fix` — она же исправляет проведённое (SPEC-29 §24, ADR-085). */
function StockInForm({ product, fix, onCancel, onDone }: { product: Product; fix?: FixMode<StockInDoc>; onCancel: () => void; onDone: () => void }) {
  const [qty, setQty] = useState(fix ? qtyInput(product.unit, fix.doc.qty) : '')
  const [price, setPrice] = useState(
    fix ? amountInput(fix.doc.price_tyiyn) : product.last_purchase_price_tyiyn != null ? somInput(product.last_purchase_price_tyiyn) : '',
  )
  const [comment, setComment] = useState(fix?.doc.comment ?? '')
  const [opId, setOpId] = useState(newOpId)
  const act = useAction()
  const q = stockInQty(product, qty)
  const p = parseSom(price)
  const unitWord = product.unit === 'ml' ? 'л' : product.unit === 'g' ? 'кг' : 'шт'
  const perWord = product.unit === 'ml' && product.container_ml ? `за канистру ${formatLiters(product.container_ml)}` : product.unit === 'g' ? 'за кг' : 'за шт'
  const missing = missingWithFocus(
    [q !== null, `количество, ${unitWord}`, '#stockin-qty'],
    [p !== null && p >= 0, 'цену закупки', '#stockin-price'],
    [Boolean(comment.trim()), 'комментарий — откуда товар', '#stockin-comment'],
    [!fix || Boolean(fix.comment.trim()), 'что и почему исправляете', '#correction-comment'],
  )

  const save = () =>
    void act.run(async () => {
      if (q === null || p === null) return
      if (fix) {
        const out = await fix.save({ ...fix.doc, qty: q, price_tyiyn: p, comment: comment.trim() })
        if (!out) return
        toast(fixedText(out))
        onDone()
        return
      }
      const out = await post<StockIn>('/stock-ins', { op_id: opId, product_id: product.id, qty: q, price_tyiyn: p, comment: comment.trim() })
      toast(`Оприходовано № ${out.number}: ${qtyText(out.unit, out.qty)}`)
      setOpId(newOpId())
      setQty('')
      setComment('')
      onDone()
    })

  return (
    <div className="flex flex-col gap-3">
      {fix ? (
        <FixBanner fix={fix} />
      ) : (
        <p className="text-sm text-slate-600">Товар пришёл без накладной и без денег. Он ляжет на склад по цене закупки; владелец получит уведомление.</p>
      )}
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label={`Количество, ${unitWord}`} required>
          <input id="stockin-qty" autoFocus={!fix} inputMode={product.unit === 'piece' ? 'numeric' : 'decimal'} value={qty} onChange={(e) => setQty(e.target.value)} />
        </Field>
        <Field label={`Цена закупки ${perWord}, с`} required>
          <input id="stockin-price" inputMode="decimal" value={price} onChange={(e) => setPrice(e.target.value)} />
        </Field>
      </div>
      <Field label="Комментарий: откуда товар" required>
        <input id="stockin-comment" value={comment} onChange={(e) => setComment(e.target.value)} placeholder="Например: привёз владелец со склада" />
      </Field>
      {q !== null && p !== null && (
        <div className="text-sm text-slate-700" data-testid="stockin-total">
          На сумму {formatSom(stockInCost(product, q, p))}
        </div>
      )}
      <Missing items={missing} />
      <ErrorBox error={act.error ?? fix?.error} />
      <div className="flex justify-end gap-2">
        <Button variant="secondary" onClick={onCancel}>
          {fix ? 'Отмена' : 'Закрыть'}
        </Button>
        <Button disabled={act.busy || Boolean(fix?.busy) || missing.length > 0} onClick={save}>
          {fix ? FIX_SUBMIT : 'Оприходовать'}
        </Button>
      </div>
    </div>
  )
}

export function StockInModal({ product, onClose, onDone }: { product: Product; onClose: () => void; onDone: () => void }) {
  const [reversing, setReversing] = useState<{ doc: StockIn; comment: string; opId: string } | null>(null)
  const [fixing, setFixing] = useState<StockIn | null>(null)
  const history = useLoad(() => get<StockIn[]>(`/stock-ins${qs({ product_id: product.id })}`), [product.id])
  // Отметки «исправлен → / исправление» — одним запросом на список.
  const marks = useLoad(() => loadCorrections('stock_in').then(correctionMarks), [])
  const act = useAction()
  const changed = () => {
    history.reload()
    marks.reload()
    onDone()
  }

  return (
    <Modal title={`${fixing ? `Исправить оприходование № ${fixing.number}` : 'Оприходовать'}: ${product.name}`} onClose={onClose}>
      <div className="flex flex-col gap-3">
        {fixing ? (
          <FixDraft<StockInDoc> key={fixing.id} kind="stock_in" id={fixing.id} onCancel={() => setFixing(null)}>
            {(fix) => (
              <StockInForm
                product={product}
                fix={fix}
                onCancel={() => setFixing(null)}
                onDone={() => {
                  setFixing(null)
                  changed()
                }}
              />
            )}
          </FixDraft>
        ) : (
          <StockInForm product={product} onCancel={onClose} onDone={changed} />
        )}
        {(history.data ?? []).length > 0 && (
          <div className="border-t border-slate-200 pt-3">
            <div className="mb-1 text-sm font-medium">Оприходования этого товара</div>
            <ul className="flex flex-col gap-1 text-sm">
              {(history.data ?? []).map((d) => (
                <li key={d.id} className="flex flex-wrap items-center justify-between gap-2">
                  <span>
                    № {d.number} · {formatDateTime(d.created_at)} · {qtyText(d.unit, d.qty)} · {formatSom(d.cost_tyiyn)} · {d.user_name}
                    {d.comment && <span className="text-slate-500"> — {d.comment}</span>}{' '}
                    {d.reversal_of && <Badge tone="amber">сторно</Badge>}
                    {d.reversed_by && <Badge tone="rose">сторнировано</Badge>} <CorrectionBadges mark={marks.data?.get(d.id)} />
                  </span>
                  {!d.reversal_of && !d.reversed_by && (
                    <span className="flex gap-1">
                      <FixButton
                        small
                        onClick={() => {
                          setReversing(null)
                          setFixing(d)
                        }}
                      />
                      <Button variant="ghost" className="px-2 py-1 text-xs" onClick={() => setReversing({ doc: d, comment: '', opId: newOpId() })}>
                        Сторно
                      </Button>
                    </span>
                  )}
                </li>
              ))}
            </ul>
          </div>
        )}
        {reversing && (
          <div className="flex flex-col gap-2 rounded-md border border-rose-200 bg-rose-50 p-3">
            <div className="text-sm">
              Сторно оприходования № {reversing.doc.number}: {qtyText(reversing.doc.unit, reversing.doc.qty)} уйдёт со склада.
            </div>
            <Field label="Причина" required>
              <input autoFocus value={reversing.comment} onChange={(e) => setReversing({ ...reversing, comment: e.target.value })} />
            </Field>
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setReversing(null)}>
                Отмена
              </Button>
              <Button
                variant="danger"
                disabled={act.busy || !reversing.comment.trim()}
                onClick={() =>
                  void act.run(async () => {
                    await post(`/stock-ins/${reversing.doc.id}/reverse`, { op_id: reversing.opId, comment: reversing.comment.trim() })
                    setReversing(null)
                    changed()
                  })
                }
              >
                Сторнировать
              </Button>
            </div>
          </div>
        )}
      </div>
    </Modal>
  )
}
