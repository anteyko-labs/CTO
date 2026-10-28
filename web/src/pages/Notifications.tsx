import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { Pager, usePageState, usePager } from '../components/Pager'
import { Badge, Card, Empty, ErrorBox, Loading, PageHeader } from '../components/ui'
import { get, post, qs } from '../lib/api'
import { formatDateTime } from '../lib/format'
import { PAGE_SIZES } from '../lib/paging'
import { useLoad } from '../lib/hooks'
import type { NotificationItem, NotificationsOut } from '../lib/types'

type Tone = 'rose' | 'amber' | 'slate'
type Group = 'money' | 'stock' | 'people'

/** Насколько важно и к чему относится: деньги, склад или люди и доступы. */
const KINDS: Record<string, { tone: Tone; group: Group }> = {
  'sale.below_cost': { tone: 'rose', group: 'money' },
  'sale.return_over_paid': { tone: 'rose', group: 'money' },
  'sale.credit_limit_exceeded': { tone: 'rose', group: 'money' },
  'sale.offline_rejected': { tone: 'rose', group: 'money' },
  'receipt.cost_above_price': { tone: 'rose', group: 'stock' },
  'shift.handover': { tone: 'slate', group: 'money' },
  'cash.reverse': { tone: 'amber', group: 'money' },
  'cash.transfer_reverse': { tone: 'amber', group: 'money' },
  'debt.adjust': { tone: 'amber', group: 'money' },
  'debt.old': { tone: 'amber', group: 'money' },
  'party.limit_request': { tone: 'amber', group: 'money' },
  'sale.return': { tone: 'slate', group: 'money' },
  'sale.price_override': { tone: 'amber', group: 'money' },
  'sale.stale_price': { tone: 'amber', group: 'money' },
  'product.prices': { tone: 'amber', group: 'stock' },
  'receipt.reverse': { tone: 'amber', group: 'stock' },
  'revision.post': { tone: 'slate', group: 'stock' },
  'oil.transfer_reverse': { tone: 'amber', group: 'stock' },
  'gift.rule': { tone: 'slate', group: 'stock' },
  'user.create': { tone: 'slate', group: 'people' },
  'user.update': { tone: 'slate', group: 'people' },
  'user.permissions': { tone: 'amber', group: 'people' },
}

const kindOf = (n: NotificationItem) => KINDS[n.action] ?? { tone: 'slate' as Tone, group: 'money' as Group }

/** Недостача при сдаче кассы — тоже тревожная, хотя само действие обычное. */
const toneOf = (n: NotificationItem): Tone => (n.action === 'shift.handover' && /недостач/i.test(n.details) ? 'rose' : kindOf(n).tone)

const DOT: Record<Tone, string> = { rose: 'bg-rose-500', amber: 'bg-amber-500', slate: 'bg-slate-400' }

const FILTERS = [
  ['all', 'Все'],
  ['important', 'Важные'],
  ['money', 'Деньги'],
  ['stock', 'Склад'],
  ['people', 'Люди'],
] as const

type Filter = (typeof FILTERS)[number][0]

/** Куда перейти по уведомлению: чек, приход, клиент; если связи нет — ссылки нет. */
function linkOf(n: NotificationItem): { to: string; label: string } | null {
  if (n.action === 'product.prices') return { to: '/products', label: 'Товары' }
  if (n.action === 'revision.post') return { to: '/revision', label: 'Ревизии' }
  if (n.action.startsWith('cash.') || n.action === 'shift.handover') return { to: '/shift', label: 'Кассы' }
  if (!n.entity_id) return null
  if (n.action.startsWith('sale.')) return { to: `/sales/${n.entity_id}`, label: 'Открыть чек' }
  if (n.action.startsWith('receipt.')) return { to: `/receipts/${n.entity_id}`, label: 'Открыть приход' }
  if (n.action === 'party.limit_request' || n.action === 'debt.adjust' || n.action === 'debt.old') return { to: `/clients/${n.entity_id}`, label: 'Карточка' }
  return null
}

/** Что происходило без владельца: цены, возвраты, сторно, долги, доступы. */
export default function Notifications() {
  const [filter, setFilter] = useState<Filter>('all')
  const grouped = filter !== 'all'
  // «Все» — страницу режет сервер, число событий — в ответе. Разделы сервер не различает: для них
  // берём последние 500 событий и режем на страницы здесь.
  const paged = usePageState('notifications', 'all')
  const data = useLoad(() => get<NotificationsOut & { total?: number }>(`/notifications${qs({ limit: paged.size, offset: paged.page * paged.size })}`), [paged.page, paged.size])
  const recent = useLoad(() => (grouped ? get<NotificationsOut>('/notifications?limit=500') : Promise.resolve(null)), [grouped])

  // Открыли экран — значит прочитали.
  useEffect(() => {
    if (data.data) void post('/notifications/seen').catch(() => undefined)
  }, [data.data])

  const last = recent.data?.items ?? []
  const inGroup = (n: NotificationItem, f: Filter) => (f === 'important' ? toneOf(n) !== 'slate' : kindOf(n).group === f)
  const filtered = grouped ? last.filter((n) => inGroup(n, filter)) : []
  const groupPager = usePager(filtered, 'notifications', filter)
  const allTotal = data.data?.total ?? data.data?.items.length ?? 0
  const items = grouped ? groupPager.shown : (data.data?.items ?? [])
  const total = grouped ? groupPager.total : allTotal
  const pager = grouped ? groupPager : paged
  const loading = grouped ? recent.loading && !recent.data : data.loading && !data.data
  /** Число на кнопке раздела: у «Всех» — весь журнал, у разделов — среди последних 500. */
  const count = (f: Filter): number | null => (f === 'all' ? allTotal : recent.data ? last.filter((n) => inGroup(n, f)).length : null)

  return (
    <div>
      <PageHeader title="Уведомления" subtitle={data.data && data.data.unseen > 0 ? `Новых: ${data.data.unseen}` : undefined} />
      <div className="mb-3 flex flex-wrap gap-2">
        {FILTERS.map(([key, label]) => (
          <button
            key={key}
            type="button"
            aria-pressed={filter === key}
            className={`min-h-9 rounded-full border px-3 text-sm ${filter === key ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white hover:bg-slate-50'}`}
            onClick={() => setFilter(key)}
          >
            {label}
            {count(key) !== null && <span className={`ml-1 tabular-nums ${filter === key ? 'text-sky-100' : 'text-slate-400'}`}>{count(key)}</span>}
          </button>
        ))}
      </div>
      <Card>
        <ErrorBox error={data.error ?? recent.error} />
        {loading ? (
          <Loading />
        ) : total === 0 ? (
          <Empty icon="bell">{allTotal === 0 ? 'Пока ничего важного не происходило' : 'В этом разделе уведомлений нет'}</Empty>
        ) : (
          <ul className={`-mx-4 -mt-4 divide-y divide-slate-100 overflow-hidden rounded-lg ${total > PAGE_SIZES[0] ? '' : '-mb-4'}`}>
            {items.map((n) => {
              const tone = toneOf(n)
              const link = linkOf(n)
              return (
                <li key={n.id} className={`flex gap-3 px-4 py-3 ${n.new ? 'bg-sky-50' : ''}`}>
                  <span className={`mt-1.5 h-2.5 w-2.5 shrink-0 rounded-full ${DOT[tone]}`} aria-hidden="true" />
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-0.5">
                      <span className={`${n.new ? 'font-semibold' : 'font-medium'} ${tone === 'rose' ? 'text-rose-700' : ''}`}>
                        {n.title}
                        {n.new && (
                          <span className="ml-2 align-middle">
                            <Badge tone="sky">новое</Badge>
                          </span>
                        )}
                      </span>
                      <span className="whitespace-nowrap text-xs text-slate-500">{formatDateTime(n.at)}</span>
                    </div>
                    {n.details && <div className="text-sm text-slate-600">{n.details}</div>}
                    <div className="mt-0.5 flex flex-wrap gap-x-3 text-xs text-slate-500">
                      {n.user_name && <span>{n.user_name}</span>}
                      {link && (
                        <Link to={link.to} className="text-sky-700 underline">
                          {link.label}
                        </Link>
                      )}
                    </div>
                  </div>
                </li>
              )
            })}
          </ul>
        )}
        <Pager page={pager.page} size={pager.size} total={total} onPage={pager.setPage} onSize={pager.setSize} label="Страницы уведомлений" />
      </Card>
    </div>
  )
}
