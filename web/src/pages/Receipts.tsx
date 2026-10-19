// Список приходов за период (SPEC-03): накладные поставщиков и оприходования без поставщика вместе,
// по дате, с отбором по виду (SPEC-29 §8, ADR-086).
import { useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { Pager, usePagedLoad } from '../components/Pager'
import { CorrectionBadges } from '../components/Correction'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, PageHeader, Table } from '../components/ui'
import { correctionMarks, loadCorrections } from '../lib/corrections'
import { useLoad } from '../lib/hooks'
import { qs } from '../lib/api'
import { formatDateTime, formatSom, shiftDate, todayBishkek } from '../lib/format'
import type { ReceiptListItem } from '../lib/types'

type Kind = 'all' | 'receipt' | 'stock_in'
const KINDS: [Kind, string][] = [
  ['all', 'Все'],
  ['receipt', 'От поставщиков'],
  ['stock_in', 'Оприходования'],
]

/** Адрес документа списка: накладная или оприходование. */
const hrefOf = (r: ReceiptListItem) => (r.kind === 'stock_in' ? `/stock-ins/${r.id}` : `/receipts/${r.id}`)

export default function Receipts() {
  const navigate = useNavigate()
  const [params, setParams] = useSearchParams()
  const kindParam = params.get('kind')
  const kind: Kind = kindParam === 'receipt' || kindParam === 'stock_in' ? kindParam : 'all'
  const setKind = (k: Kind) => setParams(k === 'all' ? {} : { kind: k }, { replace: true })
  const [to, setTo] = useState(todayBishkek)
  const [from, setFrom] = useState(() => shiftDate(todayBishkek(), -30))
  // Страницу режет сервер; смена периода или вида — снова первая страница.
  const list = usePagedLoad<ReceiptListItem>(`/receipts${qs({ from, to, kind })}`, 'receipts')
  // Исправленные документы и исправления — одним запросом на вид (SPEC-29 §24).
  const fixes = useLoad(
    () =>
      Promise.all([loadCorrections('receipt'), loadCorrections('stock_in')]).then(([a, b]) => correctionMarks([...a, ...b])),
    [from, to],
  )
  const newReceipt = () => navigate(kind === 'stock_in' ? '/receipts/new?kind=stock_in' : '/receipts/new')
  /** Отметки строки: вид, сторно, оплата накладной, исправления. */
  const badges = (r: ReceiptListItem) => (
    <>
      {r.kind === 'stock_in' && <Badge tone="sky">Оприходование</Badge>}
      {r.reversal_of && <Badge tone="amber">сторно</Badge>}
      {r.reversed && <Badge tone="rose">{r.kind === 'stock_in' ? 'сторнировано' : 'сторнирован'}</Badge>}
      {r.kind !== 'stock_in' &&
        !r.reversal_of &&
        !r.reversed &&
        ((r.on_debt_tyiyn ?? 0) > 0 ? <Badge tone="amber">в долг поставщику</Badge> : <Badge tone="green">оплачен</Badge>)}
      <CorrectionBadges mark={fixes.data?.get(r.id)} />
    </>
  )
  /** Откуда товар: поставщик и его документ или товары и комментарий оприходования. */
  const source = (r: ReceiptListItem) =>
    r.kind === 'stock_in' ? (
      <>
        {r.goods}
        {r.comment && <span className="ml-2 text-slate-500">— {r.comment}</span>}
      </>
    ) : (
      <>
        {r.supplier_name ?? '—'}
        {r.supplier_doc && <span className="ml-2 text-slate-500">док. {r.supplier_doc}</span>}
      </>
    )

  return (
    <div>
      <PageHeader title="Приход" actions={<Button onClick={newReceipt}>Новый приход</Button>} />
      <Card>
        <div className="mb-4 grid grid-cols-2 gap-3 sm:max-w-2xl sm:grid-cols-3">
          <Field label="С">
            <input type="date" value={from} onChange={(e) => setFrom(e.target.value)} />
          </Field>
          <Field label="По">
            <input type="date" value={to} onChange={(e) => setTo(e.target.value)} />
          </Field>
          <Field label="Показать">
            <select id="receipts-kind" value={kind} onChange={(e) => setKind(e.target.value as Kind)}>
              {KINDS.map(([k, label]) => (
                <option key={k} value={k}>
                  {label}
                </option>
              ))}
            </select>
          </Field>
        </div>
        <ErrorBox error={list.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : list.data && list.total === 0 ? (
          <Empty icon="incoming" action={<Button onClick={newReceipt}>Новый приход</Button>}>
            {kind === 'stock_in' ? 'Оприходований за период нет' : 'Приходов за период нет'}
          </Empty>
        ) : (
          list.data && (
            <>
              <ul className="-mx-4 divide-y divide-slate-100 border-y border-slate-100 md:hidden">
                {list.items.map((r) => (
                  <li key={`${r.kind}-${r.id}`}>
                    <button type="button" className="flex w-full items-start gap-3 px-4 py-2.5 text-left hover:bg-slate-50" onClick={() => navigate(hrefOf(r))}>
                      <div className="min-w-0 flex-1">
                        <div className="truncate font-semibold">
                          № {r.number} · {r.kind === 'stock_in' ? r.goods : (r.supplier_name ?? 'без поставщика')}
                        </div>
                        <div className="mt-0.5 truncate text-xs text-slate-500">
                          {formatDateTime(r.created_at)}
                          {r.kind === 'stock_in' ? r.comment && ` · ${r.comment}` : r.supplier_doc && ` · док. ${r.supplier_doc}`} · {r.user_name}
                        </div>
                        <div className="mt-1 flex flex-wrap gap-1">{badges(r)}</div>
                      </div>
                      <div className="shrink-0 text-right font-bold tabular-nums">{formatSom(r.total_tyiyn)}</div>
                    </button>
                  </li>
                ))}
              </ul>
              <div className="hidden md:block">
                <Table head={['№', 'Дата', 'Поставщик / откуда', 'Сумма', 'Принял', '']}>
                  {list.items.map((r) => (
                    <tr key={`${r.kind}-${r.id}`} data-testid="receipt-row" className="cursor-pointer hover:bg-slate-50" onClick={() => navigate(hrefOf(r))}>
                      <td className="px-2 py-2 font-medium">{r.number}</td>
                      <td className="whitespace-nowrap px-2 py-2">{formatDateTime(r.created_at)}</td>
                      <td className="px-2 py-2">{source(r)}</td>
                      <td className="whitespace-nowrap px-2 py-2 text-right">{formatSom(r.total_tyiyn)}</td>
                      <td className="px-2 py-2">{r.user_name}</td>
                      <td className="px-2 py-2">
                        <div className="flex flex-wrap gap-1">{badges(r)}</div>
                      </td>
                    </tr>
                  ))}
                </Table>
              </div>
              <Pager page={list.page} size={list.size} total={list.total} onPage={list.setPage} onSize={list.setSize} label="Страницы приходов" />
            </>
          )
        )}
      </Card>
    </div>
  )
}
