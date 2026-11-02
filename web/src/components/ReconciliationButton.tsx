// Акт сверки на экране (SPEC-10, задача 003 Н9): период с быстрыми кнопками, сальдо, строки со
// ссылками на документы, обороты и вывод; «Печать» — тот же акт A4. Кнопка — по праву «Акт сверки».
// Тот же экран — в кабинете юрлица (`ReconciliationView`, задача 003).
import { useState } from 'react'
import { Link } from 'react-router-dom'
import { get, qs } from '../lib/api'
import { ACT_PERIODS, actDocLink, actLines, actPeriod, actVerdict, pngDataUrl, printReconciliation, type ActPeriod, type DebtDocSettings, type Reconciliation, type Seller } from '../lib/debtDocs'
import { formatSom, todayBishkek } from '../lib/format'
import { useLoad } from '../lib/hooks'
import { useCan } from '../lib/permissions'
import { reconciliationSheet } from '../lib/exportCols'
import { downloadXlsx, fileName } from '../lib/exportTable'
import { Button, ErrorBox, Field, Loading, Modal, Table } from './ui'

const dateRu = (iso: string) => {
  const [y, m, d] = iso.slice(0, 10).split('-')
  return `${d}.${m}.${y}`
}

/** Окно акта: открывается кнопкой или из меню «⋯». «Скачать» — акт в .xlsx (задача 003 Н9, Н13); `onDownload` заменяет выгрузку. */
export function ReconciliationModal({
  partyId,
  onClose,
  onDownload,
}: {
  partyId: string
  onClose: () => void
  onDownload?: (act: Reconciliation) => void
}) {
  const settings = useLoad(() => get<DebtDocSettings>('/settings/debt-docs'), [])
  return (
    <Modal title="Акт сверки взаимных расчётов" onClose={onClose} wide>
      <ReconciliationView
        source={`/parties/${partyId}/reconciliation`}
        // Пока настройка не пришла — период не выбран (без настройки — всё время, как раньше).
        defaultPeriod={settings.loading && !settings.data ? null : (settings.data?.act_period ?? 'all')}
        seller={settings.data?.seller}
        ownerSigned={Boolean(settings.data?.has_owner_signature)}
        error={settings.error}
        links
        onClose={onClose}
        onDownload={onDownload}
      />
    </Modal>
  )
}

/**
 * Акт сверки — один экран для программы и кабинета юрлица (задача 003): быстрые периоды, «С» и
 * «По», сальдо, строки, обороты, вывод; «Печать» — тот же акт A4, «Скачать» — .xlsx. `source` —
 * адрес акта (`?from=&to=` добавляется сам); `links` — строки ведут на чек и приход (в кабинете
 * документов точки нет); `onClose` — кнопка «Закрыть» окна.
 */
export function ReconciliationView({
  source,
  defaultPeriod,
  seller,
  ownerSigned = false,
  error,
  links = false,
  onClose,
  onDownload,
}: {
  source: string
  defaultPeriod: ActPeriod | null
  seller: Seller | undefined
  /** Подпись руководителя задана — встаёт в печатный акт (задача 008); кабинет клиента — без неё. */
  ownerSigned?: boolean
  error?: string | null
  links?: boolean
  onClose?: () => void
  onDownload?: (act: Reconciliation) => void
}) {
  const today = todayBishkek()
  // Выбор на экране; пока его нет — период по умолчанию из «Настройки → Документы о долге».
  const [chosen, setChosen] = useState<{ period: ActPeriod | 'custom'; from: string; to: string } | null>(null)
  const shown = chosen ?? (defaultPeriod === null ? null : { period: defaultPeriod, ...actPeriod(defaultPeriod, today) })
  const period = shown?.period ?? null
  const from = shown?.from ?? ''
  const to = shown?.to ?? today
  const ready = shown !== null
  const act = useLoad(() => (ready ? get<Reconciliation>(`${source}${qs({ from, to })}`) : Promise.resolve(null)), [source, from, to, ready])
  const choose = (kind: ActPeriod) => setChosen({ period: kind, ...actPeriod(kind, today) })
  const setFrom = (v: string) => setChosen({ period: 'custom', from: v, to })
  const setTo = (v: string) => setChosen({ period: 'custom', from, to: v })
  const a = act.data
  const us = seller?.name ?? ''
  const lines = a ? actLines(a) : []
  // Тот же акт, что на экране и в печати: строки `actLines`, деньги — числами из тыйынов (I1).
  const download = onDownload ?? ((x: Reconciliation) => downloadXlsx(fileName(`Акт сверки ${x.name}`, `${dateRu(x.from)}–${dateRu(x.to)}`), [reconciliationSheet(x, us)]))

  return (
    <div className="flex flex-col gap-3" data-testid="reconciliation">
      <div className="flex flex-wrap gap-2">
        {ACT_PERIODS.map((p) => (
          <Button key={p.key} variant={period === p.key ? 'primary' : 'secondary'} aria-pressed={period === p.key} onClick={() => choose(p.key)}>
            {p.label}
          </Button>
        ))}
      </div>
      <div className="grid grid-cols-2 gap-3">
        <Field label="С" hint="Пусто — с первой операции">
          <input type="date" value={from} onChange={(e) => setFrom(e.target.value)} />
        </Field>
        <Field label="По">
          <input type="date" value={to} onChange={(e) => setTo(e.target.value)} />
        </Field>
      </div>
      <ErrorBox error={act.error ?? error ?? null} />
      {!a ? (
        !act.error && <Loading />
      ) : (
        <>
          <div className="text-sm text-slate-600">
            Между {us || 'точкой'} и {a.name}
            {a.inn ? `, ИНН ${a.inn}` : ''} — с {dateRu(a.from)} по {dateRu(a.to)}
          </div>
          <Table head={['Дата', 'Документ', 'Дебет', 'Кредит']}>
            {lines.map((l, i) => {
              const strong = l.kind !== 'row'
              const href = links ? actDocLink(l) : null
              return (
                <tr key={i} className={strong ? 'bg-slate-50 font-medium' : ''} data-testid={`act-${l.kind}`}>
                  <td className="whitespace-nowrap px-2 py-2">{l.kind === 'row' ? dateRu(l.date) : ''}</td>
                  <td className="px-2 py-2">
                    {href ? (
                      <Link className="text-sky-700 hover:underline" to={href} onClick={onClose}>
                        {l.document}
                      </Link>
                    ) : (
                      l.document
                    )}
                    {l.comment && <div className="text-xs text-slate-500">{l.comment}</div>}
                  </td>
                  <td className="whitespace-nowrap px-2 py-2 text-right">{l.debit_tyiyn ? formatSom(l.debit_tyiyn) : ''}</td>
                  <td className="whitespace-nowrap px-2 py-2 text-right">{l.credit_tyiyn ? formatSom(l.credit_tyiyn) : ''}</td>
                </tr>
              )
            })}
          </Table>
          {a.rows.length === 0 && <div className="text-sm text-slate-500">За период движений долга не было.</div>}
          <div className="font-medium" data-testid="act-verdict">
            {actVerdict(a, us)}
          </div>
          <div className="text-xs text-slate-500">Дебет — в пользу точки (отгрузка покупателю, оплата поставщику), кредит — в пользу {a.role === 'supplier' ? 'поставщика' : 'клиента'}.</div>
        </>
      )}
      <div className="flex flex-wrap justify-end gap-2">
        {onClose && (
          <Button variant="secondary" onClick={onClose}>
            Закрыть
          </Button>
        )}
        <Button variant="secondary" disabled={!a} data-testid="act-download" onClick={() => a && download(a)}>
          Скачать
        </Button>
        <Button disabled={!a || !seller} onClick={() => a && seller && void (ownerSigned ? pngDataUrl('/settings/debt-docs/signature') : Promise.resolve(null)).then((sig) => printReconciliation(a, seller, sig))}>
          Печать
        </Button>
      </div>
    </div>
  )
}

/** Кнопка «Акт сверки» — владельцу и администратору с правом «Акт сверки». */
export function ReconciliationButton({ partyId }: { partyId: string }) {
  const [open, setOpen] = useState(false)
  const can = useCan()
  if (!can('reconciliation')) return null
  return (
    <>
      <Button variant="secondary" onClick={() => setOpen(true)}>
        Акт сверки
      </Button>
      {open && <ReconciliationModal partyId={partyId} onClose={() => setOpen(false)} />}
    </>
  )
}
