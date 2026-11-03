// Кабинет поставщика (задача 028, ADR-188): сводка долга, накладные с оплатой и файлами, деньги,
// возвраты, акт сверки. Только просмотр; поставщик может лишь приложить файл к своей накладной.
import { useRef, useState } from 'react'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, Table, toast } from '../../components/ui'
import { get, newOpId, qs, upload } from '../../lib/api'
import { Pager, usePager } from '../../components/Pager'
import { ExportButton } from '../../components/ExportButton'
import { listSheet, periodText } from '../../lib/exportTable'
import { INVOICE_COLS, invoiceRows, invoiceStatusText, invoiceTitle, invoiceTotal, type SupplierCabinetInvoice } from '../../lib/cabinetExport'
import { prepareFile } from '../../lib/photo'
import { qtyText } from '../../lib/units'
import { formatDateTime, formatSom, todayBishkek } from '../../lib/format'
import { useAction, useLoad } from '../../lib/hooks'

export interface SupplierSummary {
  debt_tyiyn: number
  advance_tyiyn: number
  unpaid_count: number
  unpaid_tyiyn: number
  branches: {
    branch_id: string
    branch_name: string
    debt_tyiyn: number
    advance_tyiyn: number
    unpaid_count: number
    unpaid_tyiyn: number
  }[]
}

interface Money {
  at: string
  branch_name: string
  kind: 'payment' | 'defect' | 'return' | 'adjust'
  amount_tyiyn: number
  reversed: boolean
  applied: { number: number | null; amount_tyiyn: number }[]
}

interface ReturnDoc {
  at: string
  branch_name: string
  kind: 'return' | 'defect'
  number: number
  settle: string
  amount_tyiyn: number
  reversed: boolean
  lines: {
    name: string
    qty: number
    unit: 'piece' | 'ml' | 'g' | 'mm'
    amount_tyiyn: number
  }[]
}

const MONEY_KIND: Record<Money['kind'], string> = {
  payment: 'Оплата',
  defect: 'Зачёт брака',
  return: 'Возврат товара',
  adjust: 'Правка',
}

const SETTLE: Record<string, string> = {
  debt: 'в счёт долга',
  money: 'деньгами',
  replace: 'заменой',
  credit: 'зачётом',
  cash: 'деньгами',
}

function usePeriod() {
  const [from, setFrom] = useState(() => {
    const [y, m, d] = todayBishkek().split('-').map(Number)
    return new Date(Date.UTC(y, m - 1, d - 90)).toISOString().slice(0, 10)
  })
  const [to, setTo] = useState(todayBishkek())
  const fields = (
    <>
      <Field label="С">
        <input type="date" value={from} onChange={(e) => setFrom(e.target.value)} />
      </Field>
      <Field label="По">
        <input type="date" value={to} onChange={(e) => setTo(e.target.value)} />
      </Field>
    </>
  )
  return { from, to, fields }
}

/** Сводка вверху: наш долг поставщику (или его аванс) и неоплаченные накладные. */
export function SupplierHeader({ s }: { s: SupplierSummary | null | undefined }) {
  if (!s) return null
  return (
    <Card className="flex flex-wrap gap-x-8 gap-y-2" data-testid="supplier-summary">
      <div>
        <div className="text-xs text-slate-500">Мы должны вам</div>
        <div className={`text-2xl font-bold ${s.debt_tyiyn > 0 ? 'text-amber-700' : ''}`}>{formatSom(s.debt_tyiyn)}</div>
        {s.branches.length > 1 && (
          <div className="text-xs text-slate-600">
            {s.branches.map((b) => (
              <div key={b.branch_id}>
                {b.branch_name}: {formatSom(b.debt_tyiyn)}
                {b.advance_tyiyn > 0 && `, аванс ${formatSom(b.advance_tyiyn)}`}
              </div>
            ))}
          </div>
        )}
      </div>
      {s.advance_tyiyn > 0 && (
        <div>
          <div className="text-xs text-slate-500">Аванс (заплачено вперёд)</div>
          <div className="text-2xl font-bold">{formatSom(s.advance_tyiyn)}</div>
        </div>
      )}
      <div>
        <div className="text-xs text-slate-500">Не оплачено накладных</div>
        <div className="font-medium">
          {s.unpaid_count} на {formatSom(s.unpaid_tyiyn)}
        </div>
      </div>
    </Card>
  )
}

function statusTone(i: SupplierCabinetInvoice): 'rose' | 'amber' | 'green' | 'slate' {
  if (i.reversed) return 'slate'
  if (i.status === 'unpaid') return 'rose'
  if (i.status === 'partial') return 'amber'
  return 'green'
}

/** Приложить файл к своей накладной: снимок сжимается, PDF — как есть. */
function AttachButton({ invoice, onDone }: { invoice: SupplierCabinetInvoice; onDone: () => void }) {
  const input = useRef<HTMLInputElement>(null)
  const { busy, error, run } = useAction()
  return (
    <div className="flex flex-col gap-1">
      <input
        ref={input}
        type="file"
        accept="image/jpeg,image/png,image/webp,application/pdf"
        className="hidden"
        data-testid="attach-input"
        onChange={(e) => {
          const files = Array.from(e.target.files ?? [])
          e.target.value = ''
          if (!files.length) return
          void run(async () => {
            for (const f of files) {
              const p = await prepareFile(f)
              await upload(`/supplier-cabinet/invoices/${invoice.id}/files${qs({ op_id: newOpId(), name: p.name })}`, p.blob, p.type)
            }
            toast('Файл приложен')
            onDone()
          })
        }}
      />
      <Button variant="secondary" className="min-h-[34px] self-start px-3 py-1 text-xs" disabled={busy} onClick={() => input.current?.click()}>
        {busy ? 'Загружаю…' : 'Приложить файл'}
      </Button>
      <ErrorBox error={error} />
    </div>
  )
}

function Invoices() {
  const { from, to, fields } = usePeriod()
  const list = useLoad(() => get<SupplierCabinetInvoice[]>(`/supplier-cabinet/invoices${qs({ from, to })}`), [from, to])
  const rows = list.data ?? []
  const live = rows.filter((r) => !r.reversed)
  const pager = usePager(rows, 'cabinet.invoices', `${from}:${to}`)
  return (
    <div className="flex flex-col gap-3">
      <Card className="flex flex-wrap items-end gap-3">
        {fields}
        <div className="text-sm text-slate-600">
          Накладных: {live.length} на {formatSom(live.reduce((a, r) => a + r.total_tyiyn, 0))}
        </div>
        <div className="ml-auto">
          <ExportButton
            file="Накладные"
            period={periodText(from, to)}
            disabled={rows.length === 0}
            sheets={() => [
              listSheet('Накладные', INVOICE_COLS, invoiceRows(rows), {
                filter: [`период ${periodText(from, to)}`],
                total: invoiceTotal(rows),
              }),
            ]}
          />
        </div>
      </Card>
      <ErrorBox error={list.error} />
      {list.loading && !list.data ? (
        <Loading />
      ) : rows.length === 0 ? (
        <Empty>За этот период накладных нет</Empty>
      ) : (
        pager.shown.map((i) => (
          <Card key={i.id} className={`flex flex-col gap-2 ${i.reversed ? 'opacity-60' : ''}`} data-testid="supplier-invoice">
            <div className="flex flex-wrap items-baseline justify-between gap-2">
              <div className="font-medium">
                {invoiceTitle(i)} · {formatDateTime(i.created_at)}
                {i.branch_name && <span className="text-slate-500"> · {i.branch_name}</span>}
              </div>
              <div className="font-semibold">{formatSom(i.total_tyiyn)}</div>
            </div>
            <div className="flex flex-wrap items-center gap-2 text-sm text-slate-600">
              <Badge tone={statusTone(i)}>{invoiceStatusText(i)}</Badge>
              {!i.reversed && i.status !== 'now' && (
                <span>
                  оплачено {formatSom(i.paid_tyiyn)} · осталось {formatSom(i.left_tyiyn)}
                </span>
              )}
            </div>
            <ul className="text-sm">
              {i.lines.map((l, k) => (
                <li key={k} className="flex justify-between gap-3 border-t border-slate-100 py-1">
                  <span>
                    {l.name} <span className="text-slate-500">· {qtyText(l.unit, l.qty)}</span>
                  </span>
                  <span className="whitespace-nowrap">{formatSom(l.amount_tyiyn)}</span>
                </li>
              ))}
            </ul>
            {i.files.length > 0 && (
              <div className="flex flex-wrap gap-2 text-sm">
                {i.files.map((f) => (
                  <a
                    key={f.id}
                    href={`/api/v1/supplier-cabinet/invoices/${i.id}/files/${f.id}`}
                    target="_blank"
                    rel="noreferrer"
                    className="rounded border border-slate-300 px-2 py-1 text-sky-700 hover:bg-slate-50"
                  >
                    {f.content_type === 'application/pdf' ? '📄' : '🖼'} {f.name || 'файл'}
                    {f.source === 'supplier' && <span className="text-slate-500"> · ваш</span>}
                  </a>
                ))}
              </div>
            )}
            {!i.reversed && <AttachButton invoice={i} onDone={list.reload} />}
          </Card>
        ))
      )}
      <Pager page={pager.page} size={pager.size} total={pager.total} onPage={pager.setPage} onSize={pager.setSize} label="Страницы накладных" />
    </div>
  )
}

function MoneyList() {
  const { from, to, fields } = usePeriod()
  const list = useLoad(() => get<Money[]>(`/supplier-cabinet/payments${qs({ from, to })}`), [from, to])
  const rows = list.data ?? []
  const several = rows.some((r) => r.branch_name)
  return (
    <div className="flex flex-col gap-3">
      <Card className="flex flex-wrap items-end gap-3">
        {fields}
        <div className="text-sm text-slate-600">Получено: {formatSom(rows.filter((r) => !r.reversed && r.kind === 'payment').reduce((a, r) => a + r.amount_tyiyn, 0))}</div>
      </Card>
      <ErrorBox error={list.error} />
      <Card className="p-0">
        {list.loading && !list.data ? (
          <Loading />
        ) : rows.length === 0 ? (
          <Empty>За этот период оплат нет</Empty>
        ) : (
          <Table head={['Когда', 'Что', 'Сумма', 'Закрыли накладные', ...(several ? ['Филиал'] : [])]}>
            {rows.map((r, i) => (
              <tr key={i} className={r.reversed ? 'text-slate-400 line-through' : ''}>
                <td className="whitespace-nowrap px-2 py-2">{formatDateTime(r.at)}</td>
                <td className="px-2 py-2">
                  {MONEY_KIND[r.kind]}
                  {r.reversed && <span className="ml-1 no-underline">(отменено)</span>}
                </td>
                <td className="whitespace-nowrap px-2 py-2">{formatSom(r.amount_tyiyn)}</td>
                <td className="px-2 py-2 text-sm">
                  {r.applied.length === 0 ? 'аванс' : r.applied.map((a) => `${a.number ? `приход № ${a.number}` : 'старый долг'} — ${formatSom(a.amount_tyiyn)}`).join('; ')}
                </td>
                {several && <td className="px-2 py-2">{r.branch_name}</td>}
              </tr>
            ))}
          </Table>
        )}
      </Card>
    </div>
  )
}

function Returns() {
  const { from, to, fields } = usePeriod()
  const list = useLoad(() => get<ReturnDoc[]>(`/supplier-cabinet/returns${qs({ from, to })}`), [from, to])
  const rows = list.data ?? []
  return (
    <div className="flex flex-col gap-3">
      <Card className="flex flex-wrap items-end gap-3">{fields}</Card>
      <ErrorBox error={list.error} />
      {list.loading && !list.data ? (
        <Loading />
      ) : rows.length === 0 ? (
        <Empty>За этот период возвратов нет</Empty>
      ) : (
        rows.map((r, i) => (
          <Card key={i} className={`flex flex-col gap-2 ${r.reversed ? 'opacity-60' : ''}`}>
            <div className="flex flex-wrap items-baseline justify-between gap-2">
              <div className="font-medium">
                {r.kind === 'defect' ? 'Брак' : 'Возврат'} № {r.number} · {formatDateTime(r.at)}
                {r.branch_name && <span className="text-slate-500"> · {r.branch_name}</span>}
              </div>
              <div className="font-semibold">{formatSom(r.amount_tyiyn)}</div>
            </div>
            <div className="text-sm text-slate-600">
              {SETTLE[r.settle] ?? r.settle}
              {r.reversed && <Badge tone="slate">отменён</Badge>}
            </div>
            <ul className="text-sm">
              {r.lines.map((l, k) => (
                <li key={k} className="flex justify-between gap-3 border-t border-slate-100 py-1">
                  <span>
                    {l.name} <span className="text-slate-500">· {qtyText(l.unit, l.qty)}</span>
                  </span>
                  <span className="whitespace-nowrap">{formatSom(l.amount_tyiyn)}</span>
                </li>
              ))}
            </ul>
          </Card>
        ))
      )}
    </div>
  )
}

export type SupplierTab = 'invoices' | 'money' | 'returns' | 'act'

export const SUPPLIER_TABS: readonly (readonly [SupplierTab, string])[] = [
  ['invoices', 'Накладные'],
  ['money', 'Деньги'],
  ['returns', 'Возвраты'],
  ['act', 'Акт сверки'],
]

export function SupplierTabBody({ tab }: { tab: SupplierTab }) {
  if (tab === 'invoices') return <Invoices />
  if (tab === 'money') return <MoneyList />
  if (tab === 'returns') return <Returns />
  return null
}
