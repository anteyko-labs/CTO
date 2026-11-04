// Список приходов за период (SPEC-03): накладные поставщиков и оприходования без поставщика вместе,
// по дате, с отбором по виду (SPEC-29 §8, ADR-086). Вверху — «Накладные по фото» из бота и с экрана
// (SPEC-30 §9): «Открыть» заполняет «Новый приход». Поиск накладной (задача 002, П6): № накладной
// поставщика, поставщик, товар или сумма — за всё время; отбор «с фото», значок фото в строке.
// Приём аккумуляторов на кассе — за наличные и в оплату чека — строками того же списка (задача 025).
import { useEffect, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { Pager, usePagedLoad } from '../components/Pager'
import { ExportButton } from '../components/ExportButton'
import { cappedWarning, fetchAllPages, listSheet, periodText, totalBy, xl, type ExportCol } from '../lib/exportTable'
import { CorrectionBadges } from '../components/Correction'
import { ReceiptDraftsBlock } from '../components/ReceiptDrafts'
import { AcceptancesWaitingBlock } from '../components/Acceptances'
import type { AcceptanceList } from '../lib/acceptances'
import { Badge, Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, Modal, PageHeader, Table } from '../components/ui'
import { Icon } from '../components/icons'
import { correctionMarks, loadCorrections } from '../lib/corrections'
import type { DraftList } from '../lib/receiptDrafts'
import { useLoad } from '../lib/hooks'
import { useCan } from '../lib/permissions'
import { get, qs } from '../lib/api'
import type { EntryDraft } from '../lib/entryDraft'
import { formatDateTime, formatSom, plural, shiftDate, todayBishkek } from '../lib/format'
import { batteryText, isBattery, receiptHref, receiptKindText, receiptsQuery } from '../lib/receiptSearch'
import { qtyText } from '../lib/units'
import type { BatteryIntakeCard, ReceiptListItem } from '../lib/types'

type Kind = 'all' | 'receipt' | 'stock_in' | 'battery'
const KINDS: [Kind, string][] = [
  ['all', 'Все'],
  ['receipt', 'От поставщиков'],
  ['stock_in', 'Оприходования'],
  ['battery', 'Приём аккумуляторов'],
]
const isKind = (k: string | null): k is Kind => KINDS.some(([x]) => x === k)

export default function Receipts() {
  const navigate = useNavigate()
  const [params, setParams] = useSearchParams()
  const kindParam = params.get('kind')
  const kind: Kind = isKind(kindParam) ? kindParam : 'all'
  const setKind = (k: Kind) => setParams(k === 'all' ? {} : { kind: k }, { replace: true })
  const [to, setTo] = useState(todayBishkek)
  const [from, setFrom] = useState(() => shiftDate(todayBishkek(), -30))
  // Поиск уходит на сервер через полсекунды после ввода; с поиском период не действует.
  const [search, setSearch] = useState('')
  const [q, setQ] = useState('')
  useEffect(() => {
    const t = window.setTimeout(() => setQ(search.trim()), 400)
    return () => window.clearTimeout(t)
  }, [search])
  const [withPhoto, setWithPhoto] = useState(false)
  // Страницу режет сервер; смена периода, вида или поиска — снова первая страница.
  const list = usePagedLoad<ReceiptListItem>(`/receipts${qs(receiptsQuery({ from, to, kind, q, photo: withPhoto }))}`, 'receipts')
  // Исправленные документы и исправления — одним запросом на вид (SPEC-29 §24).
  const fixes = useLoad(
    () =>
      Promise.all([loadCorrections('receipt'), loadCorrections('stock_in')]).then(([a, b]) => correctionMarks([...a, ...b])),
    [from, to],
  )
  // Приход и оприходование — отдельные права администратора (ADR-097).
  const can = useCan()
  const mayAdd = can('receipts') || can('stock_in')
  // Приём аккумуляторов за наличные открывается окном на месте, остальное — своей страницей.
  const [intake, setIntake] = useState<string | null>(null)
  const open = (r: ReceiptListItem) => {
    const href = receiptHref(r)
    if (href) navigate(href)
    else setIntake(r.id)
  }
  const newReceipt = () => navigate(kind === 'stock_in' || !can('receipts') ? '/receipts/new?kind=stock_in' : '/receipts/new')
  // Накладные по фото — с правом «Приход» (ADR-097, ADR-098).
  const drafts = useLoad(() => (can('receipts') ? get<DraftList>('/receipt-drafts') : Promise.resolve(null)), [])
  // Незавершённые документы этого входа (задача 030).
  const entries = useLoad(
    () =>
      Promise.all(
        (['stock_in', 'receipt'] as const).map((k) =>
          can(k === 'stock_in' ? 'stock_in' : 'receipts') ? get<EntryDraft | null>(`/entry-drafts/${k}`) : Promise.resolve(null),
        ),
      ),
    [],
  )
  // «Ждут приёмки» на планшете — с суммами, право «Приход» (SPEC-39 §2).
  const acceptances = useLoad(() => (can('receipts') ? get<AcceptanceList>('/acceptances?status=waiting') : Promise.resolve(null)), [])
  /** Отметки строки: вид, сторно, оплата накладной, исправления. */
  const badges = (r: ReceiptListItem) => (
    <>
      {r.kind === 'stock_in' && <Badge tone="sky">Оприходование</Badge>}
      {isBattery(r) && <Badge tone={(r.grams ?? 0) < 0 ? 'amber' : 'slate'}>{receiptKindText(r)}</Badge>}
      {r.reversal_of && <Badge tone="amber">сторно</Badge>}
      {r.reversed && <Badge tone="rose">{reversedText(r)}</Badge>}
      {r.kind !== 'stock_in' &&
        !isBattery(r) &&
        !r.reversal_of &&
        !r.reversed &&
        // Как в самой накладной: не оплачена, частично, оплачена (проверка 20.10.2026).
        ((r.on_debt_tyiyn ?? 0) <= 0 || (r.debt_left_tyiyn ?? 0) <= 0 ? (
          <Badge tone="green">оплачен</Badge>
        ) : (r.debt_left_tyiyn ?? 0) < (r.on_debt_tyiyn ?? 0) ? (
          <Badge tone="amber">частично</Badge>
        ) : (
          <Badge tone="amber">не оплачен</Badge>
        ))}
      <CorrectionBadges mark={fixes.data?.get(r.id)} />
    </>
  )
  /** Откуда товар: поставщик и его документ или товары и комментарий оприходования. */
  const source = (r: ReceiptListItem) =>
    isBattery(r) ? (
      <>
        {batteryText(r)}
        {r.comment && <span className="ml-2 text-slate-500">— {r.comment}</span>}
      </>
    ) : r.kind === 'stock_in' ? (
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
      <PageHeader
        title="Приход"
        actions={
          <>
            {/* Все строки отбора, а не страница (задача 003 Н13). */}
            <ExportButton
              file={kind === 'stock_in' ? 'Оприходования' : kind === 'battery' ? 'Приём аккумуляторов' : 'Приход'}
              period={q ? undefined : periodText(from, to)}
              disabled={list.total === 0}
              sheets={async () => {
                const all = await fetchAllPages<ReceiptListItem>(`/receipts${qs(receiptsQuery({ from, to, kind, q, photo: withPhoto }))}`)
                return [
                  listSheet('Приход', RECEIPT_COLS, all.items, {
                    filter: [
                      q ? `поиск «${q}» за всё время` : `период ${periodText(from, to)}`,
                      kind !== 'all' && (KINDS.find(([k]) => k === kind)?.[1] ?? ''),
                      withPhoto && 'с фото',
                    ],
                    total: totalBy(RECEIPT_COLS, { '№': 'Итого', Сумма: xl.money(all.items.reduce((a, r) => a + (r.reversal_of || r.reversed ? 0 : r.total_tyiyn), 0)) }),
                    warning: cappedWarning(all),
                  }),
                ]
              }}
            />
            {mayAdd && <Button onClick={newReceipt}>Новый приход</Button>}
          </>
        }
      />
      <div className="mb-4 flex flex-col gap-2">
        <ErrorBox error={drafts.error ?? acceptances.error ?? entries.error} />
        {entries.data?.map(
          (d) =>
            d && (
              <div key={d.kind} className="flex flex-wrap items-center gap-3 rounded-md bg-sky-50 px-3 py-2 text-sm text-sky-900" data-testid="entry-draft-banner">
                <span>
                  {d.kind === 'stock_in' ? 'Незавершённое оприходование' : 'Незавершённый приход'}: {d.body.lines.length} {plural(d.body.lines.length, ['строка', 'строки', 'строк'])}
                </span>
                <Button variant="secondary" onClick={() => navigate(d.kind === 'stock_in' ? '/receipts/new?kind=stock_in' : '/receipts/new')}>
                  Продолжить
                </Button>
              </div>
            ),
        )}
        <AcceptancesWaitingBlock
          list={acceptances.data}
          onChanged={() => {
            acceptances.reload()
            drafts.reload()
          }}
        />
        <ReceiptDraftsBlock
          items={drafts.data?.items ?? []}
          onOpen={(id) => navigate(`/receipts/new?draft=${id}`)}
          onChanged={drafts.reload}
        />
      </div>
      <Card>
        <div className="mb-3 flex flex-wrap items-end gap-3">
          <div className="min-w-0 flex-1 sm:max-w-md">
            <Field label="Найти накладную">
              <input
                id="receipts-search"
                type="search"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder="№ накладной, поставщик, товар или сумма"
              />
            </Field>
          </div>
          <Checkbox label="с фото" checked={withPhoto} onChange={setWithPhoto} />
        </div>
        <div className="mb-4 grid grid-cols-2 gap-3 sm:max-w-2xl sm:grid-cols-3">
          <Field label="С">
            <input type="date" value={from} disabled={Boolean(q)} onChange={(e) => setFrom(e.target.value)} />
          </Field>
          <Field label="По">
            <input type="date" value={to} disabled={Boolean(q)} onChange={(e) => setTo(e.target.value)} />
          </Field>
          <Field label="Показать">
            <select id="receipts-kind" value={kind} onChange={(e) => setKind(e.target.value as Kind)}>
              {/* Накладные — по праву «Приход товара», оприходования — «Оприходование» (задача 003). */}
              {/* Приём аккумуляторов — тому, кто видит «Приход» (задача 025). */}
              {KINDS.filter(([k]) => k === 'all' || (k === 'battery' ? mayAdd : can(k === 'receipt' ? 'receipts' : 'stock_in'))).map(([k, label]) => (
                <option key={k} value={k}>
                  {label}
                </option>
              ))}
            </select>
          </Field>
        </div>
        {q && <div className="mb-2 text-xs text-slate-500">Поиск за всё время</div>}
        <ErrorBox error={list.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : list.data && list.total === 0 ? (
          <Empty icon="incoming" action={mayAdd && <Button onClick={newReceipt}>Новый приход</Button>}>
            {q || withPhoto
              ? 'Ничего не нашлось'
              : kind === 'stock_in'
                ? 'Оприходований за период нет'
                : kind === 'battery'
                  ? 'Приёмов аккумуляторов за период нет'
                  : 'Приходов за период нет'}
          </Empty>
        ) : (
          list.data && (
            <>
              <ul className="-mx-4 divide-y divide-slate-100 border-y border-slate-100 md:hidden">
                {list.items.map((r) => (
                  <li key={`${r.kind}-${r.id}`}>
                    <button type="button" className="flex w-full items-start gap-3 px-4 py-2.5 text-left hover:bg-slate-50" onClick={() => open(r)}>
                      <div className="min-w-0 flex-1">
                        <div className="truncate font-semibold">
                          № {r.number} · {isBattery(r) ? batteryText(r) : r.kind === 'stock_in' ? r.goods : (r.supplier_name ?? 'без поставщика')}
                          {r.files > 0 && <PhotoMark r={r} onOpen={() => navigate(`/receipts/${r.id}?photo=1`)} />}
                        </div>
                        <div className="mt-0.5 truncate text-xs text-slate-500">
                          {formatDateTime(r.created_at)}
                          {r.kind === 'stock_in' || isBattery(r) ? r.comment && ` · ${r.comment}` : r.supplier_doc && ` · док. ${r.supplier_doc}`} · {r.user_name}
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
                    <tr key={`${r.kind}-${r.id}`} data-testid="receipt-row" className="cursor-pointer hover:bg-slate-50" onClick={() => open(r)}>
                      <td className="px-2 py-2 font-medium">{r.number}</td>
                      <td className="whitespace-nowrap px-2 py-2">{formatDateTime(r.created_at)}</td>
                      <td className="px-2 py-2">
                        {source(r)}
                        {r.files > 0 && <PhotoMark r={r} onOpen={() => navigate(`/receipts/${r.id}?photo=1`)} />}
                      </td>
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
      {intake && <IntakeModal id={intake} onClose={() => setIntake(null)} />}
    </div>
  )
}

/** Отметка отменённого документа: приём аккумуляторов отменяют, накладные сторнируют. */
function reversedText(r: ReceiptListItem): string {
  return r.kind === 'battery_intake' ? 'отменён' : r.kind === 'stock_in' ? 'сторнировано' : 'сторнирован'
}

/** Приём аккумуляторов за наличные (задача 025): что, почём, из какой кассы, отмена. Отменяют на кассе. */
function IntakeModal({ id, onClose }: { id: string; onClose: () => void }) {
  const card = useLoad(() => get<BatteryIntakeCard>(`/batteries/intakes/${id}`), [id])
  const c = card.data
  return (
    <Modal title={c ? `Приём аккумуляторов № ${c.number}` : 'Приём аккумуляторов'} onClose={onClose}>
      <ErrorBox error={card.error} />
      {!c ? (
        !card.error && <Loading />
      ) : (
        <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-sm" data-testid="battery-intake-card">
          <dt className="text-slate-500">Вес</dt>
          <dd>{qtyText('g', c.grams)}</dd>
          <dt className="text-slate-500">Цена за кг</dt>
          <dd>{formatSom(c.price_per_kg_tyiyn)}</dd>
          <dt className="text-slate-500">Сумма</dt>
          <dd className="font-semibold">{formatSom(c.amount_tyiyn)}</dd>
          <dt className="text-slate-500">Касса</dt>
          <dd>{c.account_name}</dd>
          <dt className="text-slate-500">Принял</dt>
          <dd>
            {c.user_name}, {formatDateTime(c.created_at)}
          </dd>
          {c.comment && (
            <>
              <dt className="text-slate-500">Комментарий</dt>
              <dd>{c.comment}</dd>
            </>
          )}
          {c.reversed_at && (
            <>
              <dt className="text-slate-500">Отменён</dt>
              <dd className="text-rose-700">
                {c.reversed_by}, {formatDateTime(c.reversed_at)}
                {c.reverse_comment && ` — «${c.reverse_comment}»`}
              </dd>
            </>
          )}
        </dl>
      )}
    </Modal>
  )
}

/** Значок фото в строке списка: нажатие открывает накладную сразу с фото на весь экран. */
function PhotoMark({ r, onOpen }: { r: ReceiptListItem; onOpen: () => void }) {
  return (
    <button
      type="button"
      data-testid="receipt-photo-mark"
      title={`Фото накладной: ${r.files} — открыть`}
      aria-label={`Фото накладной № ${r.number}`}
      className="ml-2 inline-flex items-center gap-0.5 rounded bg-sky-50 px-1.5 py-0.5 align-middle text-xs font-normal text-sky-700 hover:bg-sky-100"
      onClick={(e) => {
        e.stopPropagation()
        onOpen()
      }}
    >
      <Icon name="camera" className="h-3.5 w-3.5" />
      {r.files > 1 && <span>{r.files}</span>}
    </button>
  )
}

/** Оплата накладной словами — как значок в строке. */
function payState(r: ReceiptListItem): string {
  if (r.kind === 'stock_in' || isBattery(r) || r.reversal_of || r.reversed) return ''
  const onDebt = r.on_debt_tyiyn ?? 0
  const left = r.debt_left_tyiyn ?? 0
  return onDebt <= 0 || left <= 0 ? 'оплачен' : left < onDebt ? 'частично' : 'не оплачен'
}

/** Столбцы выгрузки «Прихода»: как в таблице; вид, оплата и сторно — отдельными столбцами. */
const RECEIPT_COLS: ExportCol<ReceiptListItem>[] = [
  { label: '№', value: (r) => xl.int(r.number) },
  { label: 'Дата', value: (r) => xl.dateTime(r.created_at) },
  { label: 'Вид', value: receiptKindText },
  { label: 'Поставщик / откуда', value: (r) => (isBattery(r) ? batteryText(r) : r.kind === 'stock_in' ? (r.goods ?? '') : (r.supplier_name ?? '')) },
  { label: 'Документ поставщика', value: (r) => (r.kind === 'stock_in' || isBattery(r) ? '' : r.supplier_doc) },
  { label: 'Комментарий', value: (r) => r.comment ?? '' },
  { label: 'Сумма', value: (r) => xl.money(r.total_tyiyn) },
  { label: 'Вес, кг', value: (r) => (isBattery(r) ? xl.units(r.grams ?? 0, 'g') : null) },
  { label: 'В долг', value: (r) => (r.kind === 'stock_in' || isBattery(r) ? null : xl.money(r.on_debt_tyiyn ?? 0)) },
  { label: 'Осталось оплатить', value: (r) => (r.kind === 'stock_in' || isBattery(r) ? null : xl.money(r.debt_left_tyiyn ?? 0)) },
  { label: 'Оплата', value: payState },
  { label: 'Принял', value: (r) => r.user_name },
  { label: 'Фото', value: (r) => (r.files > 0 ? xl.int(r.files) : null) },
  { label: 'Отметка', value: (r) => (r.reversal_of ? 'сторно' : r.reversed ? reversedText(r) : '') },
]
