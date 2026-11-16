import { useState } from 'react'
import { get, newOpId, post } from '../lib/api'
import { parseSom } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import { payeeHints, payeeMissing } from '../lib/expensePayee'
import type { ExpenseArticle, ExpensePayees, Product } from '../lib/types'
import { asUnit, parseQty, qtyText, unitShort } from '../lib/units'
import { ProductPicker } from './ProductPicker'
import { Button, ErrorBox, Field, Missing, Modal, toast } from './ui'

type Mode = 'money' | 'goods'

/** Мелкий расход прямо с кассы: деньги из кассы (SPEC-06) или свой товар на нужды (задача 038). */
export function QuickExpense({ onClose }: { onClose: () => void }) {
  const [mode, setMode] = useState<Mode>('money')
  const tab = (m: Mode, label: string, testid: string) => (
    <button
      type="button"
      data-testid={testid}
      aria-pressed={mode === m}
      onClick={() => setMode(m)}
      className={`rounded-md px-3 py-1.5 text-sm ${mode === m ? 'bg-sky-600 text-white' : 'text-slate-700 hover:bg-slate-100'}`}
    >
      {label}
    </button>
  )
  return (
    <Modal title="Мелкий расход из кассы" onClose={onClose}>
      <div className="mb-3 inline-flex rounded-lg border border-slate-200 bg-white p-0.5" role="group" aria-label="Что расходуем">
        {tab('money', 'Деньги из кассы', 'quick-mode-money')}
        {tab('goods', 'Свой товар', 'quick-mode-goods')}
      </div>
      {mode === 'money' ? <MoneyForm onClose={onClose} /> : <OwnUseForm onClose={onClose} />}
    </Modal>
  )
}

/** Свой товар на нужды магазина: уходит со склада по себестоимости, деньги кассы не трогаются. */
function OwnUseForm({ onClose }: { onClose: () => void }) {
  const [product, setProduct] = useState<Product | null>(null)
  const [qty, setQty] = useState('')
  const [comment, setComment] = useState('')
  const { busy, error, run } = useAction()
  // Один op_id на открытую форму: повтор после потерянного ответа не спишет дважды.
  const [opId] = useState(newOpId)
  const unit = asUnit(product?.unit) ?? 'piece'
  const q = product ? parseQty(unit, qty) : null
  const notFilled = missingWithFocus([Boolean(product), 'товар', '#own-use-product'], [q !== null && q > 0, 'количество', '#own-use-qty'])
  const save = () =>
    run(async () => {
      if (!product || q === null || q <= 0) throw new Error('Неверное количество')
      await post('/own-use', { op_id: opId, product_id: product.id, qty: q, comment: comment.trim() })
      toast(`Списано на нужды: ${product.name} — ${qtyText(unit, q)}`)
      onClose()
    })
  const pick = (p: Product) => {
    setProduct(p)
    setQty(p.unit === 'piece' ? '1' : '')
  }
  return (
    <div className="flex flex-col gap-3">
      <p className="text-sm text-slate-600">Товар со склада на нужды магазина: уходит из остатка, деньги из кассы не берутся. В прибыли — строка «Товар на нужды магазина».</p>
      {product ? (
        <div className="flex items-center justify-between gap-2 rounded-lg border border-slate-200 px-3 py-2" data-testid="own-use-picked">
          <div>
            <div className="font-medium">{product.name}</div>
            <div className="text-xs text-slate-500">на складе {qtyText(unit, product.stock_qty)}</div>
          </div>
          <Button variant="secondary" onClick={() => setProduct(null)}>
            Другой товар
          </Button>
        </div>
      ) : (
        <ProductPicker inputId="own-use-product" placeholder="Сканируйте или найдите товар" onPick={pick} />
      )}
      {product && (
        <Field label={`Количество, ${unitShort(unit)}`} required>
          <input id="own-use-qty" autoFocus inputMode="decimal" value={qty} onChange={(e) => setQty(e.target.value)} />
        </Field>
      )}
      <Field label="Комментарий" hint="На что взяли">
        <input id="own-use-comment" maxLength={500} value={comment} onChange={(e) => setComment(e.target.value)} />
      </Field>
      <Missing items={notFilled} />
      <ErrorBox error={error} />
      <div className="flex justify-end gap-2">
        <Button variant="secondary" onClick={onClose}>
          Отмена
        </Button>
        <Button disabled={busy || notFilled.length > 0} onClick={() => void save()}>
          Списать
        </Button>
      </div>
    </div>
  )
}

/** Деньги из кассы по статье расходов (SPEC-06). */
function MoneyForm({ onClose }: { onClose: () => void }) {
  const articles = useLoad(() => get<ExpenseArticle[]>('/expense-articles'), [])
  // «Кому» (задача 003 Н12): спрашивать ли — настройка «Отчёт о прибыли».
  const payees = useLoad(() => get<ExpensePayees>('/expenses/payees'), [])
  const mode = payees.data?.mode ?? 'optional'
  const [form, setForm] = useState({ article: '', sum: '', comment: '', payee: '' })
  const { busy, error, run } = useAction()
  // Один op_id на открытую форму: повтор после потерянного ответа не задвоит расход.
  const [opId] = useState(newOpId)
  const active = (articles.data ?? []).filter((a) => a.active && !a.owner_only)
  const notFilled = missingWithFocus(
    [Boolean(form.article), 'статью', '#quick-article'],
    [Boolean(form.sum.trim()), 'сумму', '#quick-sum'],
    [!payeeMissing(mode, form.payee), 'кому', '#quick-payee'],
  )

  const save = () =>
    run(async () => {
      const sum = parseSom(form.sum)
      if (sum === null || sum <= 0) throw new Error('Неверная сумма')
      await post('/expenses', {
        op_id: opId,
        article_id: form.article,
        amount_tyiyn: sum,
        source: 'account',
        comment: form.comment,
        payee: form.payee.trim(),
      })
      toast('Расход записан, деньги ушли из кассы')
      onClose()
    })

  return (
    <div className="flex flex-col gap-3">
      <Field label="Статья" required>
        <select id="quick-article" value={form.article} onChange={(e) => setForm({ ...form, article: e.target.value })}>
          <option value="">— выберите —</option>
          {active.map((a) => (
            <option key={a.id} value={a.id}>
              {a.name}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Сумма, с" required>
        <input id="quick-sum" autoFocus inputMode="decimal" value={form.sum} onChange={(e) => setForm({ ...form, sum: e.target.value })} />
      </Field>
      {mode !== 'off' && (
        <Field label="Кому" required={mode === 'required'} hint="Кому отдали деньги">
          <input id="quick-payee" maxLength={100} list="quick-payees" value={form.payee} onChange={(e) => setForm({ ...form, payee: e.target.value })} />
          <datalist id="quick-payees">
            {payeeHints(payees.data?.names ?? []).map((n) => (
              <option key={n} value={n} />
            ))}
          </datalist>
        </Field>
      )}
      <Field label="Комментарий" hint="По статье «Прочее» обязателен">
        <input value={form.comment} onChange={(e) => setForm({ ...form, comment: e.target.value })} />
      </Field>
      <Missing items={notFilled} />
      <ErrorBox error={error ?? articles.error} />
      <div className="flex justify-end gap-2">
        <Button variant="secondary" onClick={onClose}>
          Отмена
        </Button>
        <Button disabled={busy || notFilled.length > 0} onClick={() => void save()}>
          Записать
        </Button>
      </div>
    </div>
  )
}
