import { useState } from 'react'
import { get, newOpId, post } from '../lib/api'
import { parseSom } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import { payeeHints, payeeMissing } from '../lib/expensePayee'
import type { ExpenseArticle, ExpensePayees } from '../lib/types'
import { Button, ErrorBox, Field, Missing, Modal, toast } from './ui'

/** Мелкий расход прямо с кассы: деньги уходят из кассы сразу (SPEC-06). */
export function QuickExpense({ onClose }: { onClose: () => void }) {
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
    <Modal title="Мелкий расход из кассы" onClose={onClose}>
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
    </Modal>
  )
}
