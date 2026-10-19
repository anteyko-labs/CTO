// Возврат товара поставщику — не брак (SPEC-29 §9): строки товар и количество, сумма по умолчанию —
// цена последнего прихода от него × количество; итог — меньше наш долг или деньги на счёт.
// Себестоимость и разницу видит только владелец.
import { useState } from 'react'
import { get, newOpId, post, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { qtyText } from '../lib/defects'
import { formatDateTime, formatSom, parseSom, somInput } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import { paySources } from '../lib/receiptPay'
import { returnAmount, returnQty } from '../lib/supplierReturn'
import type { CashAccount, Product, SupplierReturn } from '../lib/types'
import { ProductPicker } from './ProductPicker'
import { Badge, Button, Empty, ErrorBox, Field, Missing, Modal, Table, toast } from './ui'

interface Line {
  key: string
  product: Product
  qty: string
  amount: string
  /** Сумму поменяли руками — пересчёт по количеству её не трогает. */
  manual: boolean
  /** Цена последнего прихода от поставщика: за штуку, канистру или кг; null — не возил. */
  price: number | null
}

const unitWord = (p: Product) => (p.unit === 'ml' ? 'л' : p.unit === 'g' ? 'кг' : 'шт')

/** Окно возврата поставщику. */
export function SupplierReturnModal({ supplierId, supplierName, onClose, onDone }: { supplierId: string; supplierName: string; onClose: () => void; onDone: () => void }) {
  const owner = useUser().role === 'owner'
  const [lines, setLines] = useState<Line[]>([])
  const [settle, setSettle] = useState<'debt' | 'money'>('debt')
  const [account, setAccount] = useState('')
  const [comment, setComment] = useState('')
  const [opId] = useState(newOpId)
  const accounts = useLoad(() => get<CashAccount[]>('/cash/accounts'), [])
  const sources = paySources(accounts.data ?? [], owner)
  const act = useAction()

  const recalc = (l: Line, qty: string): Line => {
    const q = returnQty(l.product, qty)
    return { ...l, qty, amount: l.manual || q === null || l.price === null ? l.amount : somInput(returnAmount(l.product, q, l.price)) }
  }

  const add = (p: Product) => {
    if (lines.some((l) => l.product.id === p.id)) return
    const key = crypto.randomUUID()
    setLines((ls) => [...ls, { key, product: p, qty: p.unit === 'piece' ? '1' : '', amount: '', manual: false, price: null }])
    void get<{ price_tyiyn: number | null }>(`/supplier-returns/price${qs({ supplier_id: supplierId, product_id: p.id })}`)
      .then((r) => setLines((ls) => ls.map((l) => (l.key === key ? recalc({ ...l, price: r.price_tyiyn }, l.qty) : l))))
      .catch(() => undefined)
  }

  const parsed = lines.map((l) => ({ l, q: returnQty(l.product, l.qty), a: l.amount.trim() ? parseSom(l.amount) : null }))
  const bad = parsed.findIndex((x) => x.q === null || (x.l.amount.trim() !== '' && x.a === null))
  const total = parsed.reduce((s, x) => s + (x.a ?? 0), 0)
  const missing = missingWithFocus(
    [lines.length > 0, 'товары — отсканируйте или найдите', '[data-return-picker] input'],
    [bad < 0, `количество и сумму в строке ${bad + 1}`, `#return-qty-${bad + 1}`],
    [settle === 'debt' || Boolean(account), 'счёт, куда пришли деньги', '#return-account'],
  )

  const save = () =>
    void act.run(async () => {
      const out = await post<SupplierReturn>('/supplier-returns', {
        op_id: opId,
        supplier_id: supplierId,
        settle,
        account_id: settle === 'money' ? account : null,
        comment: comment.trim(),
        lines: parsed.map((x) => ({ product_id: x.l.product.id, qty: x.q, ...(x.a !== null ? { amount_tyiyn: x.a } : {}) })),
      })
      toast(`Возврат поставщику № ${out.number}: ${formatSom(out.amount_tyiyn)}`)
      onDone()
    })

  return (
    <Modal title={`Возврат поставщику: ${supplierName}`} wide onClose={onClose}>
      <div className="flex flex-col gap-3">
        <p className="text-sm text-slate-600">
          Товар уходит со склада. Сумма по умолчанию — цена последнего прихода от этого поставщика × количество; её можно поменять.
          Брак оформляйте на экране «Брак».
        </p>
        <div data-return-picker>
          <ProductPicker onPick={(p) => add(p)} autoFocus placeholder="Сканируйте или найдите товар, который возвращаем" />
        </div>
        {lines.length === 0 ? (
          <Empty>Добавьте товары</Empty>
        ) : (
          <Table head={['Товар', 'Количество', 'Сумма возврата, с', '']}>
            {lines.map((l, i) => (
              <tr key={l.key}>
                <td className="px-2 py-2">
                  <div className="font-medium">{l.product.name}</div>
                  <div className="text-xs text-slate-500">
                    на складе {qtyText(l.product.stock_qty, l.product.unit)}
                    {l.price !== null && ` · последний приход от него ${formatSom(l.price)}`}
                  </div>
                </td>
                <td className="px-2 py-2">
                  <label className="flex items-center gap-1 text-xs text-slate-500">
                    <input
                      id={`return-qty-${i + 1}`}
                      className="w-24"
                      inputMode={l.product.unit === 'piece' ? 'numeric' : 'decimal'}
                      value={l.qty}
                      onChange={(e) => setLines((ls) => ls.map((x) => (x.key === l.key ? recalc(x, e.target.value) : x)))}
                    />
                    {unitWord(l.product)}
                  </label>
                </td>
                <td className="px-2 py-2">
                  <input
                    id={`return-amount-${i + 1}`}
                    className="w-28"
                    inputMode="decimal"
                    placeholder={l.price === null ? 'по себестоимости' : ''}
                    value={l.amount}
                    onChange={(e) => setLines((ls) => ls.map((x) => (x.key === l.key ? { ...x, amount: e.target.value, manual: true } : x)))}
                  />
                </td>
                <td className="px-2 py-2 text-right">
                  <Button variant="ghost" className="px-2 py-1" aria-label="Убрать строку" onClick={() => setLines((ls) => ls.filter((x) => x.key !== l.key))}>
                    ✕
                  </Button>
                </td>
              </tr>
            ))}
          </Table>
        )}
        <div className="flex flex-wrap items-center gap-4 text-sm">
          <label className="flex items-center gap-1">
            <input type="radio" name="return-settle" checked={settle === 'debt'} onChange={() => setSettle('debt')} />В счёт долга поставщику
          </label>
          <label className="flex items-center gap-1">
            <input type="radio" name="return-settle" checked={settle === 'money'} onChange={() => setSettle('money')} />
            Вернул деньгами
          </label>
          {settle === 'money' && (
            <select id="return-account" value={account} onChange={(e) => setAccount(e.target.value)}>
              <option value="">— на какой счёт —</option>
              {sources.map((s) => (
                <option key={s.value} value={s.value}>
                  {s.label}
                </option>
              ))}
            </select>
          )}
          <span className="ml-auto font-semibold" data-testid="return-total">
            Итого: {formatSom(total)}
          </span>
        </div>
        <Field label="Комментарий">
          <input value={comment} onChange={(e) => setComment(e.target.value)} placeholder="Почему возвращаем" />
        </Field>
        <Missing items={missing} />
        <ErrorBox error={act.error ?? accounts.error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={act.busy || missing.length > 0} onClick={save}>
            Провести возврат
          </Button>
        </div>
      </div>
    </Modal>
  )
}

/** Возвраты поставщику в его карточке: список и сторно. */
export function SupplierReturnList({ supplierId, onChanged }: { supplierId: string; onChanged: () => void }) {
  const owner = useUser().role === 'owner'
  const list = useLoad(() => get<SupplierReturn[]>(`/supplier-returns${qs({ supplier_id: supplierId })}`), [supplierId])
  const [reversing, setReversing] = useState<{ doc: SupplierReturn; comment: string; opId: string } | null>(null)
  const act = useAction()
  const rows = list.data ?? []
  return (
    <div className="flex flex-col gap-3">
      <ErrorBox error={list.error ?? act.error} />
      {rows.length === 0 ? (
        <Empty>Возвратов этому поставщику не было</Empty>
      ) : (
        <Table head={['№', 'Дата', 'Товары', 'Сумма', 'Итог', ...(owner ? ['Разница'] : []), 'Кто', '']}>
          {rows.map((r) => (
            <tr key={r.id} className={r.reversed_by ? 'text-slate-400' : ''}>
              <td className="px-2 py-2 font-medium">
                {r.number} {r.reversal_of && <Badge tone="amber">сторно</Badge>}
                {r.reversed_by && <Badge tone="rose">отменён</Badge>}
              </td>
              <td className="whitespace-nowrap px-2 py-2">{formatDateTime(r.created_at)}</td>
              <td className="px-2 py-2">
                {r.lines.map((l) => `${l.product_name} × ${qtyText(Math.abs(l.qty), l.unit)}`).join(', ')}
                {r.comment && <div className="text-xs text-slate-500">{r.comment}</div>}
              </td>
              <td className="whitespace-nowrap px-2 py-2 tabular-nums">{formatSom(r.amount_tyiyn)}</td>
              <td className="px-2 py-2">{r.settle === 'debt' ? 'в счёт долга' : `деньгами на «${r.account_name ?? ''}»`}</td>
              {owner && <td className="whitespace-nowrap px-2 py-2 tabular-nums">{r.diff_tyiyn !== undefined ? formatSom(r.diff_tyiyn) : '—'}</td>}
              <td className="px-2 py-2">{r.user_name}</td>
              <td className="px-2 py-2 text-right">
                {!r.reversal_of && !r.reversed_by && (
                  <Button variant="ghost" className="px-2 py-1 text-xs" onClick={() => setReversing({ doc: r, comment: '', opId: newOpId() })}>
                    Сторно
                  </Button>
                )}
              </td>
            </tr>
          ))}
        </Table>
      )}
      {reversing && (
        <Modal title={`Сторно возврата № ${reversing.doc.number}`} onClose={() => setReversing(null)}>
          <div className="flex flex-col gap-3">
            <p className="text-sm text-slate-600">Товар вернётся на склад по той же стоимости, долг или деньги — обратно.</p>
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
                    await post(`/supplier-returns/${reversing.doc.id}/reverse`, { op_id: reversing.opId, comment: reversing.comment.trim() })
                    setReversing(null)
                    list.reload()
                    onChanged()
                  })
                }
              >
                Сторнировать
              </Button>
            </div>
          </div>
        </Modal>
      )}
    </div>
  )
}
