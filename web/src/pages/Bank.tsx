// Экран «Счёт» (SPEC-27 §6): безнал отдельно — поступления картой и переводом, комиссия банка,
// списания и все движения кассы «Счёт» за период. Только владелец; суммы считает сервер.
import { useState } from 'react'
import { useSearchParams } from 'react-router-dom'
import { Pager, usePager } from '../components/Pager'
import { PeriodPicker } from '../components/PeriodPicker'
import { Card, CardTitle, Empty, ErrorBox, Loading, Money, PageHeader, Table } from '../components/ui'
import { get, qs } from '../lib/api'
import { formatDate, formatDateTime, formatSom } from '../lib/format'
import { useLoad, useRangeParam } from '../lib/hooks'
import { MOVEMENT_LABELS } from '../lib/income'
import type { BankReport, CashAccount } from '../lib/types'

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
  const [range, today, setRange] = useRangeParam()
  // Любой безналичный счёт, и архивный; без выбора — основной (ADR-076).
  const [params] = useSearchParams()
  const [account, setAccount] = useState(() => params.get('account') ?? '')
  const banks = useLoad(() => get<CashAccount[]>('/cash/accounts?archived=true').then((all) => all.filter((a) => a.kind === 'bank')), [])
  const b = useLoad(
    () => get<BankReport>(`/cash/bank${qs({ from: range.from, to: range.to, account_id: account || undefined })}`),
    [range.from, range.to, account],
  )
  const data = b.data
  // Движения за период приходят вместе с итогами; страницы — здесь, смена периода — первая страница.
  const moves = usePager(data?.movements ?? [], 'bank.movements', `${account}:${range.from}:${range.to}`)
  const subtitle = range.from === range.to ? `за ${formatDate(range.from)}` : `с ${formatDate(range.from)} по ${formatDate(range.to)}`

  return (
    <div>
      <PageHeader title={data?.name ?? 'Счёт'} subtitle={`Безнал ${subtitle}: карта, перевод, комиссия банка и списания`} />
      {(banks.data ?? []).length > 1 && (
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
          <Card className="grid grid-cols-2 gap-4 sm:grid-cols-3 lg:grid-cols-6">
            <Tile label="Остаток сейчас" value={data.balance_tyiyn} />
            <Tile
              label="Картой"
              value={data.card.total_tyiyn}
              tone="text-emerald-700"
              hint={`чеки ${formatSom(data.card.sales_tyiyn)} · долги ${formatSom(data.card.repayments_tyiyn)}`}
            />
            <Tile
              label="Переводом (QR)"
              value={data.transfer.total_tyiyn}
              tone="text-emerald-700"
              hint={`чеки ${formatSom(data.transfer.sales_tyiyn)} · долги ${formatSom(data.transfer.repayments_tyiyn)}`}
            />
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
              <Table head={['День', 'Картой', 'Переводом', 'Комиссия', 'Прочее', 'Итог дня']}>
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
