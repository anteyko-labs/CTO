import { useState } from 'react'
import { Link } from 'react-router-dom'
import { Pager, usePagedLoad } from '../components/Pager'
import { CorrectionBadges, CorrectionBanner, FixButton, useCorrection } from '../components/Correction'
import { Badge, Button, Card, CardTitle, Empty, ErrorBox, Field, Loading, Missing, Modal, Money, PageHeader, Table, toast } from '../components/ui'
import { get, newOpId, post, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatDate, formatDateTime, formatSom, parseSom, todayBishkek } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import type { CashAccount, Expense, ExpenseArticle, ExpensePayees } from '../lib/types'
import { isTill } from '../lib/accounts'
import { correctionMarks, loadCorrections, numberText } from '../lib/corrections'
import { expenseDoc, expenseFields, type ExpenseDoc, type ExpenseFields } from '../lib/moneyFix'
import { payeeHints, payeeMissing } from '../lib/expensePayee'
import { ExportButton } from '../components/ExportButton'
import { cappedWarning, fetchAllPages, periodText, tableSheet, xl, type ExportCol } from '../lib/exportTable'

/** Расходы точки: из кассы, со счёта или не из денег точки (SPEC-06). */
export default function Expenses() {
  const owner = useUser().role === 'owner'
  const [from, setFrom] = useState(todayBishkek())
  const [to, setTo] = useState(todayBishkek())
  const [form, setForm] = useState<ExpenseFields>({ article: '', sum: '', account: '', date: todayBishkek(), comment: '', payee: '' })
  const articles = useLoad(() => get<ExpenseArticle[]>('/expense-articles'), [])
  // Поле «Кому» и подсказки (задача 003 Н12): спрашивать ли — настройка «Отчёт о прибыли».
  const payees = useLoad(() => get<ExpensePayees>('/expenses/payees'), [])
  const accounts = useLoad(() => get<CashAccount[]>('/cash/accounts'), [])
  // Страницу режет сервер, итог за период — отдельным запросом по всему отбору (проверка
  // 20.10.2026: раньше итог считался по первым 500 строкам).
  const list = usePagedLoad<Expense>(`/expenses${qs({ from, to })}`, 'expenses')
  const summary = useLoad(() => get<{ count: number; total_tyiyn: number }>(`/expenses/summary${qs({ from, to })}`), [from, to])
  // Отметки «исправлен → № N» / «исправление № M» — одним запросом на список.
  const corrections = useLoad(() => loadCorrections('expense'), [])
  const marks = correctionMarks(corrections.data ?? [])
  const [fixing, setFixing] = useState<string | null>(null)
  const act = useAction()
  // op_id формы: повтор после потерянного ответа не задвоит расход, после успеха — новый.
  const [opId, setOpId] = useState(newOpId)

  const active = (articles.data ?? []).filter((a) => a.active)
  const notFilled = missingWithFocus(
    [Boolean(form.article), 'статью', '#expense-article'],
    [Boolean(form.sum.trim()), 'сумму', '#expense-sum'],
    [!payeeMissing(payees.data?.mode, form.payee), 'кому', '#expense-payee'],
  )
  const total = summary.data?.total_tyiyn ?? 0
  const reload = () => {
    list.reload()
    summary.reload()
  }

  const save = () =>
    void act.run(async () => {
      const sum = parseSom(form.sum)
      if (sum === null || sum <= 0) throw new Error('Неверная сумма')
      await post('/expenses', {
        op_id: opId,
        article_id: form.article,
        amount_tyiyn: sum,
        source: 'account',
        account_id: form.account || null,
        expense_date: owner ? form.date : null,
        comment: form.comment,
        payee: form.payee.trim(),
      })
      setForm({ ...form, sum: '', comment: '', payee: '' })
      setOpId(newOpId())
      reload()
      payees.reload()
      accounts.reload()
      toast('Расход записан')
    })

  const reverse = (e: Expense) =>
    void act.run(async () => {
      if (!window.confirm(`Сторнировать расход № ${e.number} на ${formatSom(e.amount_tyiyn)}?`)) return
      await post(`/expenses/${e.id}/reverse`, { op_id: newOpId(), comment: '' })
      reload()
      accounts.reload()
      toast('Сторно записано')
    })

  return (
    <div>
      <PageHeader
        title="Расходы"
        actions={
          owner && (
            <Link to="/settings/expense-articles" className="text-sm font-medium text-sky-700 underline">
              Статьи расходов
            </Link>
          )
        }
      />

      <Card className="mb-4 flex flex-col gap-3">
        <ExpenseFormFields
          idPrefix="expense"
          form={form}
          onChange={(f) => setForm({ ...form, ...f })}
          articles={active}
          accounts={accounts.data ?? []}
          owner={owner}
          payees={payees.data}
        />
        <ErrorBox error={act.error ?? articles.error} />
        <div className="flex flex-wrap items-center justify-end gap-3">
          <Missing items={notFilled} className="mr-auto" />
          <Button disabled={act.busy || notFilled.length > 0} onClick={save}>
            Записать
          </Button>
        </div>
      </Card>

      <Card>
        <CardTitle
          actions={
            <ExportButton
              file="Расходы"
              period={periodText(from, to)}
              disabled={list.total === 0}
              sheets={async () => {
                // Все строки отбора, а не одна страница (задача 003 Н13).
                const all = await fetchAllPages<Expense>(`/expenses${qs({ from, to })}`)
                return [
                  tableSheet('Расходы', EXPORT_COLS, all.items, {
                    title: ['Расходы', `Период: ${periodText(from, to)}`],
                    total: ['Итого', null, null, null, null, xl.money(total)],
                    warning: cappedWarning(all),
                  }),
                ]
              }}
            />
          }
        >
          Записанные расходы
        </CardTitle>
        <div className="mb-4 flex flex-wrap items-end gap-3">
          <Field label="С">
            <input type="date" className="w-40" value={from} onChange={(e) => setFrom(e.target.value)} />
          </Field>
          <Field label="По">
            <input type="date" className="w-40" value={to} onChange={(e) => setTo(e.target.value)} />
          </Field>
          <div className="ml-auto text-right">
            <div className="text-xs text-slate-500">Всего за период</div>
            <Money value={total} className="text-xl font-bold" />
          </div>
        </div>
        <ErrorBox error={list.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : list.items.length === 0 ? (
          <Empty>Расходов за период нет</Empty>
        ) : (
          <Table head={['№', 'День', 'Статья', 'Кому', 'Из кассы', 'Сумма', 'Комментарий', '']}>
            {list.items.map((e) => (
              <tr key={e.id} className={e.reversal_of ? 'text-slate-400' : ''}>
                <td className="px-2 py-2 font-medium">{e.number}</td>
                <td className="whitespace-nowrap px-2 py-2">{formatDate(e.expense_date)}</td>
                <td className="px-2 py-2">
                  {e.article_name}
                  {e.reversal_of && (
                    <span className="ml-2">
                      <Badge tone="rose">сторно</Badge>
                    </span>
                  )}
                  <span className="ml-2 inline-flex flex-wrap gap-1">
                    <CorrectionBadges mark={marks.get(e.id)} />
                  </span>
                </td>
                <td className="px-2 py-2">{e.payee || '—'}</td>
                <td className="px-2 py-2">{e.source === 'outside' ? 'не из денег точки' : (e.account_name ?? '—')}</td>
                <td className="px-2 py-2 md:text-right">
                  <Money value={e.amount_tyiyn} />
                </td>
                <td className="px-2 py-2 text-slate-600">
                  {e.comment || '—'}
                  <div className="text-xs text-slate-400">
                    {e.user_name}, {formatDateTime(e.created_at)}
                  </div>
                </td>
                <td className="px-2 py-2 text-right">
                  {!e.reversal_of && !e.reversed && (
                    <div className="flex justify-end gap-2">
                      <FixButton small onClick={() => setFixing(e.id)} />
                      <Button variant="secondary" className="px-2 py-1 text-xs" disabled={act.busy} onClick={() => reverse(e)}>
                        Сторно
                      </Button>
                    </div>
                  )}
                </td>
              </tr>
            ))}
          </Table>
        )}
        <Pager page={list.page} size={list.size} total={list.total} onPage={list.setPage} onSize={list.setSize} label="Страницы расходов" />
      </Card>

      {fixing && (
        <ExpenseFix
          key={fixing}
          id={fixing}
          articles={articles.data ?? []}
          accounts={accounts.data ?? []}
          payees={payees.data}
          onClose={() => setFixing(null)}
          onDone={() => {
            setFixing(null)
            reload()
            accounts.reload()
            corrections.reload()
          }}
        />
      )}
    </div>
  )
}

/** Столбцы выгрузки «Расходов» — как в таблице экрана, сторно — отдельным столбцом. */
const EXPORT_COLS: ExportCol<Expense>[] = [
  { label: '№', value: (e) => xl.int(e.number) },
  { label: 'День', value: (e) => xl.date(e.expense_date) },
  { label: 'Статья', value: (e) => e.article_name },
  { label: 'Кому', value: (e) => e.payee ?? '' },
  { label: 'Из кассы', value: (e) => (e.source === 'outside' ? 'не из денег точки' : (e.account_name ?? '')) },
  { label: 'Сумма', value: (e) => xl.money(e.amount_tyiyn) },
  { label: 'Комментарий', value: (e) => e.comment },
  { label: 'Кто провёл', value: (e) => e.user_name },
  { label: 'Когда', value: (e) => formatDateTime(e.created_at) },
  { label: 'Отметка', value: (e) => (e.reversal_of ? 'сторно' : e.reversed ? 'сторнирован' : '') },
]

/** Поля расхода — одни для записи и для исправления. */
function ExpenseFormFields({
  idPrefix,
  form,
  onChange,
  articles,
  accounts,
  owner,
  payees,
}: {
  idPrefix: string
  form: ExpenseFields
  onChange: (f: Partial<ExpenseFields>) => void
  /** Статьи на выбор (уже отобранные: действующие и, при исправлении, статья исходного расхода). */
  articles: ExpenseArticle[]
  accounts: CashAccount[]
  owner: boolean
  /** Поле «Кому»: настройка и подсказки; пока не загружено — поле есть, необязательное. */
  payees: ExpensePayees | null | undefined
}) {
  const mode = payees?.mode ?? 'optional'
  return (
    <>
      <div className="grid gap-3 sm:grid-cols-[2fr_1fr_1fr] sm:items-end">
        <Field label="Статья" required>
          <select id={`${idPrefix}-article`} value={form.article} onChange={(e) => onChange({ article: e.target.value })}>
            <option value="">— выберите —</option>
            {articles.map((a) => (
              <option key={a.id} value={a.id}>
                {a.name}
                {a.owner_only ? ' (только владелец)' : ''}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Сумма, с" required>
          <input id={`${idPrefix}-sum`} inputMode="decimal" value={form.sum} onChange={(e) => onChange({ sum: e.target.value })} />
        </Field>
        <Field label="Из какой кассы">
          <select value={form.account} onChange={(e) => onChange({ account: e.target.value })}>
            <option value="">Касса</option>
            {accounts
              .filter((a) => !isTill(a))
              .map((a) => (
                <option key={a.id} value={a.id} disabled={!owner}>
                  {a.name}
                </option>
              ))}
          </select>
        </Field>
      </div>
      {(mode !== 'off' || form.payee !== '') && (
        <Field label="Кому" required={mode === 'required'} hint="Кому отдали деньги: человек или организация">
          <input id={`${idPrefix}-payee`} maxLength={100} list={`${idPrefix}-payees`} value={form.payee} onChange={(e) => onChange({ payee: e.target.value })} />
          <datalist id={`${idPrefix}-payees`}>
            {payeeHints(payees?.names ?? []).map((n) => (
              <option key={n} value={n} />
            ))}
          </datalist>
        </Field>
      )}
      {owner && (
        <Field label="Дата" hint="Можно поставить задним числом">
          <input type="date" className="w-44" value={form.date} onChange={(e) => onChange({ date: e.target.value })} />
        </Field>
      )}
      <Field label="Комментарий" hint="По статье «Прочее» обязателен">
        <input value={form.comment} onChange={(e) => onChange({ comment: e.target.value })} />
      </Field>
    </>
  )
}

/** «Исправить» расход (SPEC-29 §24): та же форма, заполненная исходным расходом; сторно и новый
 *  расход сервер проводит одним действием. Только владелец. */
function ExpenseFix({
  id,
  articles,
  accounts,
  payees,
  onClose,
  onDone,
}: {
  id: string
  articles: ExpenseArticle[]
  accounts: CashAccount[]
  payees: ExpensePayees | null
  onClose: () => void
  onDone: () => void
}) {
  const fix = useCorrection<ExpenseDoc>('expense', id)
  const draft = fix.draft.data
  const till = accounts.find((a) => isTill(a))?.id ?? null
  const [edited, setEdited] = useState<ExpenseFields | null>(null)
  const form = edited ?? (draft ? expenseFields(draft.doc, till) : null)
  // Статья исходного расхода остаётся на выбор, даже если её уже отключили.
  const choices = articles.filter((a) => a.active || a.id === draft?.doc.article_id)
  const notFilled = missingWithFocus(
    [Boolean(form?.article), 'статью', '#expense-fix-article'],
    [Boolean(form?.sum.trim()), 'сумму', '#expense-fix-sum'],
    [!payeeMissing(payees?.mode, form?.payee ?? ''), 'кому', '#expense-fix-payee'],
    [Boolean(fix.comment.trim()), 'что и почему исправляете', '#correction-comment'],
  )

  const doc = draft && form ? expenseDoc(draft.doc, form) : null

  const submit = async () => {
    if (!doc) return
    const c = await fix.save(doc)
    if (!c) return
    toast(`Исправлено: ${numberText(c.new_number) || 'расход'}`)
    onDone()
  }

  return (
    <Modal title={`Исправить расход ${numberText(draft?.number)}`} onClose={onClose}>
      <div className="flex flex-col gap-3">
        {!draft || !form ? (
          fix.draft.loading ? <Loading /> : null
        ) : (
          <>
            <CorrectionBanner label={draft.kind_label} number={draft.number} comment={fix.comment} onComment={fix.setComment} />
            <ExpenseFormFields
              idPrefix="expense-fix"
              form={form}
              onChange={(f) => setEdited({ ...form, ...f })}
              articles={choices}
              accounts={accounts}
              owner
              payees={payees}
            />
            {form.sum.trim() !== '' && !doc && <div className="text-xs text-rose-700">Неверная сумма</div>}
          </>
        )}
        <ErrorBox error={fix.error} />
        <div className="flex flex-wrap items-center justify-end gap-3">
          <Missing items={notFilled} className="mr-auto" />
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={fix.busy || !doc || notFilled.length > 0} onClick={() => void submit()}>
            Сохранить исправление
          </Button>
        </div>
      </div>
    </Modal>
  )
}
