// «Товары → Связать с Умагом» (SPEC-35 §3, ADR-110): сводка старой базы Умаг филиала, «Связать
// автоматически» (точный код или название при одном кандидате), «Похожие пары» товар ↔ строка Умага
// («Связать», «Не то» — больше не предлагается) и поиск строк Умага с «Отвязать». Связанная строка
// отдаёт товару свои коды — по ним его находят касса, приход, ревизия и касса без сети. Экран —
// владельцу и праву «Товары и штрихкоды»; проверяет сервер.
import { useState } from 'react'
import { Link } from 'react-router-dom'
import { Pager, usePageState } from '../components/Pager'
import { StockBackLink } from '../components/StockBackLink'
import { UmagHitModal } from '../components/Umag'
import { Badge, Button, Card, CardTitle, Empty, ErrorBox, Field, Loading, Notice, PageHeader, toast } from '../components/ui'
import { formatDateTime, formatSom } from '../lib/format'
import { useAction, useDebounced, useLoad } from '../lib/hooks'
import type { OldCatalogHit, UmagAutoLink, UmagPair, UmagStats } from '../lib/types'
import {
  autoLinkUmag,
  codesShort,
  conflictText,
  linkSummary,
  linkUmag,
  opIds,
  rejectUmagPair,
  searchUmag,
  similarityText,
  umagPairs,
  umagPer,
  umagStats,
  unlinkUmag,
} from '../lib/umagLink'

export default function UmagLink() {
  // Любое связывание меняет сводку, пары и строки — перечитываем всё.
  const [tick, setTick] = useState(0)
  const changed = () => setTick((t) => t + 1)
  const stats = useLoad(umagStats, [tick])
  const empty = stats.data !== null && stats.data.rows === 0
  return (
    <div>
      <PageHeader
        title="Связать с Умагом"
        back={<StockBackLink to="/products" label="Товары" />}
        subtitle="Коды строки старой базы Умаг становятся кодами товара: по ним товар найдут касса, приход, ревизия — и касса без сети."
      />
      {/* Пары и строки — когда известно, включён ли Умаг: выключенный отвечает 409. */}
      {!stats.data ? (
        stats.error ? <ErrorBox error={stats.error} /> : <Loading />
      ) : stats.data.enabled === false ? (
        // Умаг выключен (SPEC-36 §15): связывать нечего, коды у товаров остались.
        <Card>
          <Empty icon="products">
            Старая база Умаг выключена — связывать нечего, перенесённые коды остались у товаров. Включает владелец в{' '}
            <Link to="/settings" className="text-sky-700 underline">
              «Настройках»
            </Link>
            .
          </Empty>
        </Card>
      ) : (
        <div data-testid="umag-page" className="flex flex-col gap-4">
          <Summary stats={stats.data} error={stats.error} onChanged={changed} />
          {empty ? (
            <Card>
              <Empty icon="products">
                Список Умага не загружен. Владелец загружает выгрузку Excel в «Ревизия и товары» →{' '}
                <Link to="/revision?tab=scan" className="text-sky-700 underline">
                  «Сканер товаров»
                </Link>
                .
              </Empty>
            </Card>
          ) : (
            <>
              <Pairs tick={tick} onChanged={changed} />
              <Rows tick={tick} onChanged={changed} />
            </>
          )}
        </div>
      )}
    </div>
  )
}

function Summary({ stats, error, onChanged }: { stats: UmagStats | null; error: string | null; onChanged: () => void }) {
  const auto = useAction()
  const [ops] = useState(opIds)
  const [result, setResult] = useState<UmagAutoLink | null>(null)
  const run = () =>
    void auto.run(async () => {
      const r = await autoLinkUmag(ops.get('auto'))
      ops.done('auto')
      setResult(r)
      toast(`Связано автоматически: ${r.linked}`)
      onChanged()
    })
  const tile = (label: string, value: number | undefined, tone = '', id?: string) => (
    <div className="rounded-md border border-slate-200 px-3 py-2" data-testid={id}>
      <div className="text-xs text-slate-500">{label}</div>
      <div className={`text-xl font-semibold tabular-nums ${tone}`}>{value === undefined ? '—' : value.toLocaleString('ru-RU')}</div>
    </div>
  )
  return (
    <Card>
      <ErrorBox error={error} />
      {!stats ? (
        !error && <Loading />
      ) : (
        <div className="flex flex-col gap-3">
          <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
            {tile('Строк Умага', stats.rows, '', 'umag-rows')}
            {tile('Связано', stats.linked, 'text-emerald-700', 'umag-linked')}
            {tile('Не связано', stats.unlinked, '', 'umag-unlinked')}
            {tile('Конфликтов кодов', stats.conflicts, stats.conflicts ? 'text-amber-700' : '', 'umag-conflicts')}
          </div>
          {stats.imported_at && <div className="text-sm text-slate-500">Список загружен {formatDateTime(stats.imported_at)}.</div>}
          <div className="flex flex-wrap items-center gap-3">
            <Button data-testid="umag-auto-link" disabled={auto.busy || stats.rows === 0} onClick={run}>
              {auto.busy ? 'Связываю…' : 'Связать автоматически'}
            </Button>
            <span className="text-sm text-slate-600">
              Точный код (штрихкод или доп. код Умага = код товара) или то же название — если подходит один товар.
            </span>
          </div>
          {result && (
            <Notice tone="green">
              Связано {result.linked}: по коду {result.by_code}, по названию {result.by_name}.
            </Notice>
          )}
          <ErrorBox error={auto.error} />
        </div>
      )}
    </Card>
  )
}

const pairKey = (p: UmagPair) => `${p.umag.id}:${p.product.id}`

/** Товары без связи и похожие строки Умага без связи: «Связать» или «Не то». */
function Pairs({ tick, onChanged }: { tick: number; onChanged: () => void }) {
  const [q, setQ] = useState('')
  const query = useDebounced(q.trim(), 300)
  const pager = usePageState('umag.pairs', query)
  const load = useLoad(() => umagPairs(query, pager.page, pager.size), [query, pager.page, pager.size, tick])
  const act = useAction()
  const [ops] = useState(opIds)
  // Решённая пара уходит из списка сразу, не дожидаясь ответа на перечитывание.
  const [gone, setGone] = useState<ReadonlySet<string>>(new Set())
  const items = (load.data?.items ?? []).filter((p) => !gone.has(pairKey(p)))

  const decide = (p: UmagPair, how: 'link' | 'reject') =>
    void act.run(async () => {
      const key = `${how}:${pairKey(p)}`
      if (how === 'link') {
        const r = await linkUmag(p.umag.id, p.product.id, ops.get(key))
        toast(`«${p.product.name}» связан с Умагом: ${linkSummary(r)}`)
      } else {
        await rejectUmagPair(p.umag.id, p.product.id, ops.get(key))
        toast('Пара больше не предлагается')
      }
      ops.done(key)
      setGone((s) => new Set(s).add(pairKey(p)))
      onChanged()
    })

  return (
    <Card>
      <CardTitle>Похожие пары</CardTitle>
      <p className="mb-3 text-sm text-slate-600">
        Товары без связи и похожие на них строки Умага без связи. «Связать» — коды строки перейдут к товару; «Не то» — пара больше не
        предлагается.
      </p>
      <div className="mb-3 max-w-md">
        <Field label="Поиск пар">
          <input placeholder="Название товара или строки Умага" value={q} onChange={(e) => setQ(e.target.value)} />
        </Field>
      </div>
      <ErrorBox error={load.error ?? act.error} />
      {load.loading && !load.data ? (
        <Loading />
      ) : items.length === 0 ? (
        <Empty>{query ? 'По этому поиску похожих пар нет' : 'Похожих пар нет: всё связано или не похоже'}</Empty>
      ) : (
        <ul className="divide-y divide-slate-100">
          {items.map((p) => (
            <li
              key={pairKey(p)}
              data-testid="umag-pair-row"
              className="grid gap-2 py-3 md:grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)_auto] md:items-center md:gap-4"
            >
              <div className="min-w-0">
                <div className="text-xs uppercase text-slate-400">Товар</div>
                <div className="font-medium">{p.product.name}</div>
                <div className="text-xs text-slate-500">
                  {[p.product.brand, p.product.article, formatSom(p.product.sale_price_tyiyn)].filter(Boolean).join(' · ')}
                </div>
              </div>
              <div className="md:text-center" title="Сходство названий">
                <Badge tone="sky">{similarityText(p.similarity)}</Badge>
              </div>
              <div className="min-w-0">
                <div className="text-xs uppercase text-slate-400">Умаг</div>
                <div className="font-medium">{p.umag.name}</div>
                <div className="text-xs text-slate-500">
                  {[codesShort(p.umag), p.umag.price_tyiyn ? `${formatSom(p.umag.price_tyiyn)}${umagPer(p.umag.unit)}` : '']
                    .filter(Boolean)
                    .join(' · ')}
                </div>
              </div>
              <div className="flex gap-2 md:justify-end">
                <Button variant="secondary" data-testid="umag-pair-reject" disabled={act.busy} onClick={() => decide(p, 'reject')}>
                  Не то
                </Button>
                <Button variant="secondary" data-testid="umag-pair-link" disabled={act.busy} onClick={() => decide(p, 'link')}>
                  Связать
                </Button>
              </div>
            </li>
          ))}
        </ul>
      )}
      <Pager page={pager.page} size={pager.size} total={load.data?.total ?? 0} onPage={pager.setPage} onSize={pager.setSize} label="Страницы пар" />
    </Card>
  )
}

/** Строки Умага по названию или коду: связанную — отвязать, несвязанную — завести или связать. */
function Rows({ tick, onChanged }: { tick: number; onChanged: () => void }) {
  const [q, setQ] = useState('')
  const query = useDebounced(q.trim(), 300)
  const found = useLoad(
    () => (query.length < 2 ? Promise.resolve<OldCatalogHit[]>([]) : searchUmag({ q: query, barcode: query })),
    [query, tick],
  )
  const [open, setOpen] = useState<{ hit: OldCatalogHit; start: 'create' | 'pick' } | null>(null)
  const act = useAction()
  const [ops] = useState(opIds)

  const unlink = (h: OldCatalogHit) => {
    if (!window.confirm(`Отвязать строку Умага «${h.name}» от «${h.product_name ?? 'товара'}»? Коды останутся у товара.`)) return
    void act.run(async () => {
      const key = `unlink:${h.id}`
      await unlinkUmag(h.id, ops.get(key))
      ops.done(key)
      toast('Строка Умага отвязана, коды остались у товара')
      onChanged()
    })
  }

  const rows = found.data ?? []
  return (
    <Card>
      <CardTitle>Строки Умага</CardTitle>
      <p className="mb-3 text-sm text-slate-600">
        Найдите строку по названию или коду. Связанную можно отвязать — коды останутся у товара, пока их не уберут в карточке товара.
      </p>
      <div className="mb-3 max-w-md">
        <Field label="Найти строку Умага">
          <input placeholder="Название, штрихкод или доп. код" value={q} onChange={(e) => setQ(e.target.value)} />
        </Field>
      </div>
      <ErrorBox error={found.error ?? act.error} />
      {query.length < 2 ? null : found.loading && !found.data ? (
        <Loading />
      ) : rows.length === 0 ? (
        <Empty>В базе Умаг ничего не нашлось</Empty>
      ) : (
        <ul className="divide-y divide-slate-100">
          {rows.map((h) => (
            <li key={h.id} data-testid="umag-row" className="flex flex-wrap items-center justify-between gap-2 py-2">
              <div className="min-w-0 flex-1">
                <div className="font-medium">{h.name}</div>
                <div className="text-xs text-slate-500">
                  {[codesShort(h), h.price_tyiyn ? `${formatSom(h.price_tyiyn)}${umagPer(h.unit)}` : '', h.supplier].filter(Boolean).join(' · ')}
                </div>
                <div className="mt-1 flex flex-wrap gap-1">
                  {h.product_id ? (
                    <Badge tone="green">связана: {h.product_name ?? 'товар'}</Badge>
                  ) : h.linked_product_id ? (
                    <Badge tone="slate">код уже у товара — не связана</Badge>
                  ) : (
                    <Badge>не связана</Badge>
                  )}
                  {h.conflicts && h.conflicts.length > 0 && (
                    <Badge tone="amber">конфликт кода: {conflictText(h.conflicts)}</Badge>
                  )}
                </div>
              </div>
              <div className="flex shrink-0 gap-2">
                {h.product_id ? (
                  <Button variant="danger-outline" disabled={act.busy} onClick={() => unlink(h)}>
                    Отвязать
                  </Button>
                ) : (
                  <>
                    <Button variant="secondary" onClick={() => setOpen({ hit: h, start: 'pick' })}>
                      Это уже есть
                    </Button>
                    <Button variant="secondary" onClick={() => setOpen({ hit: h, start: 'create' })}>
                      Завести
                    </Button>
                  </>
                )}
              </div>
            </li>
          ))}
        </ul>
      )}
      {open && (
        <UmagHitModal
          hit={open.hit}
          start={open.start}
          onClose={() => setOpen(null)}
          onDone={() => {
            setOpen(null)
            onChanged()
          }}
        />
      )}
    </Card>
  )
}
