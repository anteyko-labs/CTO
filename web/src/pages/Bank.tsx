// Экран «Счёт» (SPEC-27 §6): безнал отдельно — поступления картой и переводом, комиссия банка,
// списания и все движения кассы «Счёт» за период. Только владелец; суммы считает сервер.
import { useState } from 'react'
import { Link, useSearchParams } from 'react-router-dom'
import { Pager, usePager } from '../components/Pager'
import { PeriodPicker } from '../components/PeriodPicker'
import { ExportButton } from '../components/ExportButton'
import { exportTitle, listSheet, periodText, xl, type ExportCol } from '../lib/exportTable'
import { Card, CardTitle, Empty, ErrorBox, Loading, Money, PageHeader, Table } from '../components/ui'
import { get, qs } from '../lib/api'
import { formatDate, formatDateTime, formatSom } from '../lib/format'
import { useLoad, useRangeParam } from '../lib/hooks'
import { MOVEMENT_LABELS } from '../lib/income'
import type { BankReport, CashAccount, CashMovement } from '../lib/types'
import { BranchBreakdown, BranchScope } from '../components/BranchScope'
import { useUser } from '../lib/auth'
import { branchParam, useAllBranches, type AllBranches } from '../lib/branchReport'

function Tile({ label, value, hint, tone = '' }: { label: string; value: number; hint?: string; tone?: string }) {
  return (
    <div>
      <div className="text-xs text-slate-500">{label}</div>
      <div className={`text-2xl font-bold tabular-nums ${tone}`}>{formatSom(value)}</div>
      {hint && <div className="mt-1 text-xs tabular-nums text-slate-500">{hint}</div>}
    </div>
  )
}

export default function Bank() {
  const user = useUser()
  // «Все филиалы» (SPEC-33 §3): счёт по умолчанию каждого филиала, сумма и разбивка.
  const [all, setAll] = useAllBranches(Boolean(user.several_branches))
  const [range, today, setRange] = useRangeParam()
  // Любой безналичный счёт, и архивный; без выбора — счёт по умолчанию для карты (ADR-076, ADR-077).
  const [params] = useSearchParams()
  const [account, setAccount] = useState(() => params.get('account') ?? '')
  const banks = useLoad(() => get<CashAccount[]>('/cash/accounts?archived=true').then((all) => all.filter((a) => a.kind === 'bank')), [])
  const b = useLoad(
    () =>
      get<AllBranches<BankReport>>(
        `/cash/bank${qs({ from: range.from, to: range.to, account_id: all ? undefined : account || undefined, ...branchParam(all) })}`,
      ),
    [range.from, range.to, account, all],
  )
  const data = b.data
  // Движения за период приходят вместе с итогами; страницы — здесь, смена периода — первая страница.
  const moves = usePager(data?.movements ?? [], 'bank.movements', `${account}:${range.from}:${range.to}`)
  const subtitle = range.from === range.to ? `за ${formatDate(range.from)}` : `с ${formatDate(range.from)} по ${formatDate(range.to)}`

  return (
    <div>
      <PageHeader
        title={data?.name ?? 'Счёт'}
        subtitle={`Безнал ${subtitle}: карта / QR банка, QR владельцу, комиссия банка и списания`}
        actions={
          <>
          {/* Итоги, по видам, по дням и движения — листами одного файла (задача 003 Н13). */}
          <ExportButton
            file={data?.name ?? 'Счёт'}
            period={periodText(range.from, range.to)}
            disabled={!data}
            sheets={() => (data ? bankSheets(data, [`период ${periodText(range.from, range.to)}`, all && 'все филиалы']) : [])}
          />
          {/* Сверка с выпиской Optima Business (SPEC-31): по дням и поправка комиссии. */}
          <Link to={`/bank/reconcile${qs({ from: range.from, to: range.to })}`} className="inline-flex min-h-[42px] items-center rounded-md border border-slate-300 bg-white px-4 text-sm font-medium hover:bg-slate-50" data-testid="bank-recon-link">
            Сверка с банком
          </Link>
          </>
        }
      />
      <BranchScope user={user} all={all} onChange={setAll} />
      {!all && (banks.data ?? []).length > 1 && (
        <div className="mb-3 flex flex-wrap gap-2" data-testid="bank-pick">
          {(banks.data ?? []).map((a) => {
            const on = (account || data?.account_id) === a.id
            return (
              <button
                key={a.id}
                type="button"
                aria-pressed={on}
                className={`min-h-9 rounded-md border px-3 py-1 text-sm ${on ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white hover:border-sky-500'}`}
                onClick={() => setAccount(a.id)}
              >
                {a.name}
                {!a.active && ' (архив)'}
              </button>
            )
          })}
        </div>
      )}
      <PeriodPicker value={range} today={today} onChange={setRange} />
      <ErrorBox error={b.error} />
      {b.loading && !data ? (
        <Loading />
      ) : data ? (
        <div className="flex flex-col gap-4">
          <BranchBreakdown
            parts={data.branches}
            columns={[
              { label: 'Карта', value: (x) => x.card.total_tyiyn },
              { label: 'Перевод', value: (x) => x.transfer.total_tyiyn },
              { label: 'Комиссия', value: (x) => x.fee_tyiyn },
              { label: 'На конец', value: (x) => x.closing_tyiyn },
            ]}
          />
          <Card className="grid grid-cols-2 gap-4 sm:grid-cols-3 lg:grid-cols-6">
            <Tile label="Остаток сейчас" value={data.balance_tyiyn} />
            <Tile
              label="Карта / QR банка"
              value={data.card.total_tyiyn}
              tone="text-emerald-700"
              hint={`чеки ${formatSom(data.card.sales_tyiyn)} · долги ${formatSom(data.card.repayments_tyiyn)}`}
            />
            <Tile
              label="QR владельцу"
              value={data.transfer.total_tyiyn}
              tone="text-emerald-700"
              hint={`чеки ${formatSom(data.transfer.sales_tyiyn)} · долги ${formatSom(data.transfer.repayments_tyiyn)}`}
            />
            {(data.invoice?.total_tyiyn ?? 0) !== 0 && <Tile label="По счёту" value={data.invoice?.total_tyiyn ?? 0} tone="text-emerald-700" hint="перечисления юрлиц по чекам" />}
            {(data.cash_to_bank_tyiyn ?? 0) !== 0 && (
              <Tile
                label="Наличные ↔ QR"
                value={data.cash_to_bank_tyiyn ?? 0}
                hint={(data.cash_to_bank_tyiyn ?? 0) > 0 ? 'из наличных в QR за смены' : 'из QR в наличные за смены'}
              />
            )}
            <Tile label="Комиссия банка" value={data.fee_tyiyn} tone="text-rose-700" />
            <Tile label="Списания" value={data.outgoing_tyiyn} hint={data.other_in_tyiyn ? `прочие поступления ${formatSom(data.other_in_tyiyn)}` : undefined} />
            <Tile label="На начало → на конец" value={data.closing_tyiyn} hint={`на начало ${formatSom(data.opening_tyiyn)}`} />
          </Card>

          {data.kinds.length > 0 && (
            <Card>
              <CardTitle>По видам движений</CardTitle>
              <Table head={['Вид', 'Пришло', 'Ушло']}>
                {data.kinds.map((k) => (
                  <tr key={k.kind}>
                    <td className="px-2 py-2">{MOVEMENT_LABELS[k.kind] ?? k.kind}</td>
                    <td className="px-2 py-2 text-right">
                      <Money value={k.in_tyiyn} zero="—" />
                    </td>
                    <td className="px-2 py-2 text-right">
                      <Money value={k.out_tyiyn} zero="—" />
                    </td>
                  </tr>
                ))}
              </Table>
            </Card>
          )}

          {data.days.length > 1 && (
            <Card>
              <CardTitle>По дням</CardTitle>
              <Table head={['День', 'Картой', 'Переводом', 'По счёту', 'Наличные ↔ QR', 'Комиссия', 'Прочее', 'Итог дня']}>
                {data.days.map((d) => (
                  <tr key={d.date}>
                    <td className="px-2 py-2">{formatDate(d.date)}</td>
                    <td className="px-2 py-2 text-right">
                      <Money value={d.card_tyiyn} zero="—" />
                    </td>
                    <td className="px-2 py-2 text-right">
                      <Money value={d.transfer_tyiyn} zero="—" />
                    </td>
                    <td className="px-2 py-2 text-right">
                      <Money value={d.invoice_tyiyn ?? 0} zero="—" />
                    </td>
                    <td className="px-2 py-2 text-right">
                      <Money value={d.cash_to_bank_tyiyn ?? 0} tone="sign" zero="—" />
                    </td>
                    <td className="px-2 py-2 text-right">
                      <Money value={d.fee_tyiyn} zero="—" />
                    </td>
                    <td className="px-2 py-2 text-right">
                      <Money value={d.other_tyiyn} tone="negative" zero="—" />
                    </td>
                    <td className="px-2 py-2 text-right font-semibold">
                      <Money value={d.net_tyiyn} tone="sign" />
                    </td>
                  </tr>
                ))}
              </Table>
            </Card>
          )}

          <Card>
            <CardTitle>
              Движения: {data.movements_count}
              {data.movements_count > data.movements.length && ` (показаны последние ${data.movements.length})`}
            </CardTitle>
            {data.movements.length === 0 ? (
              <Empty>За период движений нет</Empty>
            ) : (
              <Table head={['Когда', 'Что', 'Комментарий', 'Кто', 'Сумма']}>
                {moves.shown.map((m) => (
                  <tr key={m.id} data-testid="bank-movement">
                    <td className="whitespace-nowrap px-2 py-2 text-slate-500">{formatDateTime(m.created_at)}</td>
                    <td className="px-2 py-2">{MOVEMENT_LABELS[m.kind] ?? m.kind}</td>
                    <td className="px-2 py-2 text-slate-600">{m.comment || '—'}</td>
                    <td className="px-2 py-2 text-slate-600">{m.user_name}</td>
                    <td className="px-2 py-2 text-right font-medium">
                      <Money value={m.amount_tyiyn} tone="sign" />
                    </td>
                  </tr>
                ))}
              </Table>
            )}
            <Pager page={moves.page} size={moves.size} total={moves.total} onPage={moves.setPage} onSize={moves.setSize} label="Страницы движений" />
          </Card>
        </div>
      ) : null}
    </div>
  )
}

type BankDay = BankReport['days'][number]

const DAY_COLS: ExportCol<BankDay>[] = [
  { label: 'День', value: (d) => xl.date(d.date) },
  { label: 'Картой', value: (d) => xl.money(d.card_tyiyn) },
  { label: 'Переводом', value: (d) => xl.money(d.transfer_tyiyn) },
  { label: 'По счёту', value: (d) => xl.money(d.invoice_tyiyn ?? 0) },
  { label: 'Наличные ↔ QR', value: (d) => xl.money(d.cash_to_bank_tyiyn ?? 0) },
  { label: 'Комиссия', value: (d) => xl.money(d.fee_tyiyn) },
  { label: 'Прочее', value: (d) => xl.money(d.other_tyiyn) },
  { label: 'Итог дня', value: (d) => xl.money(d.net_tyiyn) },
]

const MOVE_COLS: ExportCol<CashMovement>[] = [
  { label: 'Когда', value: (m) => xl.dateTime(m.created_at) },
  { label: 'Что', value: (m) => MOVEMENT_LABELS[m.kind] ?? m.kind },
  { label: 'Комментарий', value: (m) => m.comment },
  { label: 'Кто', value: (m) => m.user_name },
  { label: 'Сумма', value: (m) => xl.money(m.amount_tyiyn) },
]

/** Экран «Счёт» в Excel: итоги, по видам движений, по дням и движения периода. */
function bankSheets(d: BankReport, filter: (string | false)[]) {
  const name = `Безнал: ${d.name}`
  const money = (label: string, v: number) => [label, xl.money(v)]
  return [
    {
      name: 'Итоги',
      columns: [{ label: 'Показатель' }, { label: 'Сумма' }],
      title: exportTitle(name, filter),
      filter: false,
      rows: [
        money('На начало', d.opening_tyiyn),
        money('Карта / QR банка', d.card.total_tyiyn),
        money('QR владельцу', d.transfer.total_tyiyn),
        money('По счёту', d.invoice?.total_tyiyn ?? 0),
        money('Наличные ↔ QR', d.cash_to_bank_tyiyn ?? 0),
        money('Комиссия банка', d.fee_tyiyn),
        money('Прочие поступления', d.other_in_tyiyn),
        money('Списания', d.outgoing_tyiyn),
        money('На конец', d.closing_tyiyn),
        money('Остаток сейчас', d.balance_tyiyn),
      ],
    },
    {
      name: 'По видам',
      columns: [{ label: 'Вид' }, { label: 'Пришло' }, { label: 'Ушло' }],
      title: exportTitle(name, filter),
      rows: d.kinds.map((k) => [MOVEMENT_LABELS[k.kind] ?? k.kind, xl.money(k.in_tyiyn), xl.money(k.out_tyiyn)]),
    },
    listSheet(name, DAY_COLS, d.days, { sheet: 'По дням', filter }),
    listSheet(name, MOVE_COLS, d.movements, {
      sheet: 'Движения',
      filter,
      warning: d.movements_count > d.movements.length ? `Выгружены последние ${d.movements.length} движений из ${d.movements_count} — сузьте период` : undefined,
    }),
  ]
}
