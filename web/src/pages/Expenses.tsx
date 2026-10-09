import { useState } from 'react'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, Missing, PageHeader, Table, toast } from '../components/ui'
import { get, newOpId, post, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatDateTime, formatSom, parseSom, todayBishkek } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import type { CashAccount, Expense, ExpenseArticle } from '../lib/types'

/** Расходы точки: из кассы, со счёта или не из денег точки (SPEC-06). */
export default function Expenses() {
  const owner = useUser().role === 'owner'
  const [from, setFrom] = useState(todayBishkek())
  const [to, setTo] = useState(todayBishkek())
  const [form, setForm] = useState({ article: '', sum: '', account: '', outside: false, date: todayBishkek(), comment: '' })
  const articles = useLoad(() => get<ExpenseArticle[]>('/expense-articles'), [])
  const accounts = useLoad(() => get<CashAccount[]>('/cash/accounts'), [])
  const list = useLoad(() => get<Expense[]>(`/expenses${qs({ from, to })}`), [from, to])
  const act = useAction()
  // op_id формы: повтор после потерянного ответа не задвоит расход, после успеха — новый.
  const [opId, setOpId] = useState(newOpId)

  const active = (articles.data ?? []).filter((a) => a.active)
  const notFilled = missingWithFocus(
    [Boolean(form.article), 'статью', '#expense-article'],
    [Boolean(form.sum.trim()), 'сумму', '#expense-sum'],
  )
  const total = (list.data ?? []).reduce((acc, e) => acc + e.amount_tyiyn, 0)

  const save = () =>
    void act.run(async () => {
      const sum = parseSom(form.sum)
      if (sum === null || sum <= 0) throw new Error('Неверная сумма')
      await post('/expenses', {
        op_id: opId,
        article_id: form.article,
        amount_tyiyn: sum,
        source: form.outside ? 'outside' : 'account',
        account_id: form.outside ? null : form.account || null,
        expense_date: owner ? form.date : null,
        comment: form.comment,
      })
      setForm({ ...form, sum: '', comment: '' })
      setOpId(newOpId())
      list.reload()
      accounts.reload()
      toast('Расход записан')
    })

  const reverse = (e: Expense) =>
    void act.run(async () => {
      if (!window.confirm(`Сторнировать расход № ${e.number} на ${formatSom(e.amount_tyiyn)}?`)) return
      await post(`/expenses/${e.id}/reverse`, { op_id: newOpId(), comment: '' })
      list.reload()
      accounts.reload()
      toast('Сторно записано')
    })

  return (
    <div>
      <PageHeader title="Расходы" />

      <Card className="mb-4 flex flex-col gap-3">
        <div className="grid gap-3 sm:grid-cols-[2fr_1fr_1fr_auto] sm:items-end">
          <Field label="Статья" required>
            <select id="expense-article" value={form.article} onChange={(e) => setForm({ ...form, article: e.target.value })}>
              <option value="">— выберите —</option>
              {active.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.name}
                  {a.owner_only ? ' (только владелец)' : ''}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Сумма, с" required>
            <input id="expense-sum" inputMode="decimal" value={form.sum} onChange={(e) => setForm({ ...form, sum: e.target.value })} />
          </Field>
          <Field label="Откуда">
            <select
              value={form.outside ? 'outside' : form.account}
              onChange={(e) =>
                setForm({ ...form, outside: e.target.value === 'outside', account: e.target.value === 'outside' ? '' : e.target.value })
              }
            >
              <option value="">Касса</option>
              {(accounts.data ?? [])
                .filter((a) => !a.is_default)
                .map((a) => (
                  <option key={a.id} value={a.id} disabled={!owner}>
                    {a.name}
                  </option>
                ))}
              {owner && <option value="outside">Не из денег точки</option>}
            </select>
          </Field>
          <Button disabled={act.busy || notFilled.length > 0} onClick={save}>
            Записать
          </Button>
        </div>
        {owner && (
          <Field label="Дата" hint="Можно поставить задним числом">
            <input type="date" className="w-44" value={form.date} onChange={(e) => setForm({ ...form, date: e.target.value })} />
          </Field>
        )}
        <Field label="Комментарий" hint="По статье «Прочее» обязателен">
          <input value={form.comment} onChange={(e) => setForm({ ...form, comment: e.target.value })} />
        </Field>
        <Missing items={notFilled} />
        <ErrorBox error={act.error ?? articles.error} />
      </Card>

      <Card>
        <div className="mb-4 flex flex-wrap items-end gap-3">
          <Field label="С">
            <input type="date" className="w-40" value={from} onChange={(e) => setFrom(e.target.value)} />
          </Field>
          <Field label="По">
            <input type="date" className="w-40" value={to} onChange={(e) => setTo(e.target.value)} />
          </Field>
          <div className="ml-auto text-right">
            <div className="text-xs text-slate-500">Всего за период</div>
            <div className="text-xl font-bold">{formatSom(total)}</div>
          </div>
        </div>
        <ErrorBox error={list.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : (list.data ?? []).length === 0 ? (
          <Empty>Расходов за период нет</Empty>
        ) : (
          <Table head={['№', 'День', 'Статья', 'Откуда', 'Сумма', 'Комментарий', '']}>
            {(list.data ?? []).map((e) => (
              <tr key={e.id} className={e.reversal_of ? 'text-slate-400' : ''}>
                <td className="px-2 py-2 font-medium">{e.number}</td>
                <td className="whitespace-nowrap px-2 py-2">{e.expense_date}</td>
                <td className="px-2 py-2">
                  {e.article_name}
                  {e.reversal_of && (
                    <span className="ml-2">
                      <Badge tone="rose">сторно</Badge>
                    </span>
                  )}
                </td>
                <td className="px-2 py-2">{e.source === 'outside' ? 'не из денег точки' : (e.account_name ?? '—')}</td>
                <td className="whitespace-nowrap px-2 py-2">{formatSom(e.amount_tyiyn)}</td>
                <td className="px-2 py-2 text-slate-600">
                  {e.comment || '—'}
                  <div className="text-xs text-slate-400">
                    {e.user_name}, {formatDateTime(e.created_at)}
                  </div>
                </td>
                <td className="px-2 py-2 text-right">
                  {!e.reversal_of && !e.reversed && (
                    <Button variant="secondary" className="px-2 py-1 text-xs" disabled={act.busy} onClick={() => reverse(e)}>
                      Сторно
                    </Button>
                  )}
                </td>
              </tr>
            ))}
          </Table>
        )}
      </Card>
    </div>
  )
}
