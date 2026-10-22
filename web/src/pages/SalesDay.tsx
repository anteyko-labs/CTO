// Чеки за день: итоги по способам оплаты, средний чек, возвраты (SPEC-04).
import { useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import { NoReceiptReturn } from '../components/NoReceiptReturn'
import { Pager, usePager } from '../components/Pager'
import { CorrectionBadges } from '../components/Correction'
import { Badge, Button, Card, Empty, ErrorBox, Loading, Money, PageHeader, Table } from '../components/ui'
import { correctionMarks, loadCorrections, type CorrectionMark } from '../lib/corrections'
import { get, qs } from '../lib/api'
import { formatSom, formatTime, shiftDate, todayBishkek } from '../lib/format'
import { useLoad } from '../lib/hooks'
import { useUser } from '../lib/auth'
import { divRound } from '../lib/money'
import type { SaleListItem, SalesDay as Day } from '../lib/types'

function Stat({ label, value, tone = '' }: { label: string; value: string; tone?: string }) {
  return (
    <Card className="p-3">
      <div className="text-xs text-slate-500">{label}</div>
      <div className={`text-lg font-semibold tabular-nums ${tone}`}>{value}</div>
    </Card>
  )
}

/** Клиент чека; безымянный (записан по телефону или ИНН) — серым «Без имени · …». */
function Client({ name, phone, fallback }: { name: string | null; phone?: string | null; fallback: string }) {
  if (!name) return <span className="text-slate-400">{fallback}</span>
  if (/^\+?[\d\s()-]+$/.test(name)) return <span className="text-slate-500">Без имени · {name}</span>
  return (
    <>
      {name}
      {phone && phone !== name && <span className="block text-xs text-slate-500">{phone}</span>}
    </>
  )
}

/** Отметки чека: возврат, в долг, исправлен / исправление (SPEC-29 §24). */
function Marks({ s, fix }: { s: SaleListItem; fix?: CorrectionMark }) {
  return (
    <>
      {s.kind === 'return' && <Badge tone="rose">{s.no_receipt ? 'возврат без чека' : 'возврат'}</Badge>}
      {s.debt_tyiyn !== 0 && <Badge tone="amber">в долг {formatSom(Math.abs(s.debt_tyiyn))}</Badge>}
      <CorrectionBadges mark={fix} />
    </>
  )
}

export default function SalesDay() {
  const [date, setDate] = useState(todayBishkek)
  const navigate = useNavigate()
  // Возврат без чека — только владелец (ADR-105).
  const owner = useUser().role === 'owner'
  const [noReceipt, setNoReceipt] = useState(false)
  const { data, error, loading, reload } = useLoad(() => get<Day>(`/sales${qs({ date })}`), [date])
  // Исправленные чеки и исправления — одним запросом на экран.
  const fixes = useLoad(() => loadCorrections('sale').then(correctionMarks), [date])
  const sold = data?.sales.filter((s) => s.kind === 'sale').reduce((acc, s) => acc + s.total_tyiyn, 0) ?? 0
  const returns = data?.sales.filter((s) => s.kind === 'return') ?? []
  const returnsSum = returns.reduce((acc, s) => acc + s.total_tyiyn, 0)
  // Чеки дня приходят целиком (итоги по ним), страницы — здесь; другой день — первая страница.
  const pager = usePager(data?.sales ?? [], 'sales.day', date)
  // Как платили — одной строкой, без нулевых способов.
  const payments = data
    ? (
        [
          ['Наличные', data.totals.cash_tyiyn],
          ['Карта / QR банка', data.totals.card_tyiyn],
          ['QR владельцу', data.totals.transfer_tyiyn],
          ['По счёту', data.totals.invoice_tyiyn ?? 0],
          ['В долг', data.totals.debt_tyiyn],
          ['Баллами', data.totals.bonus_tyiyn ?? 0],
          ['Аккумуляторами', data.totals.battery_tyiyn ?? 0],
        ] as [string, number][]
      ).filter(([, v]) => v !== 0)
    : []

  return (
    <div>
      <PageHeader
        title="Чеки за день"
        actions={
          <div className="flex items-center gap-1">
            <Button variant="secondary" aria-label="Предыдущий день" onClick={() => setDate((d) => shiftDate(d, -1))}>
              ←
            </Button>
            <input type="date" value={date} max={todayBishkek()} onChange={(e) => e.target.value && setDate(e.target.value)} />
            <Button
              variant="secondary"
              aria-label="Следующий день"
              disabled={date >= todayBishkek()}
              onClick={() => setDate((d) => shiftDate(d, 1))}
            >
              →
            </Button>
            {date !== todayBishkek() && (
              <Button variant="ghost" onClick={() => setDate(todayBishkek())}>
                Сегодня
              </Button>
            )}
            {owner && (
              <Button variant="danger-outline" className="ml-2" onClick={() => setNoReceipt(true)}>
                Возврат без чека
              </Button>
            )}
          </div>
        }
      />
      <ErrorBox error={error} />
      {data && (
        <div className="mb-4 flex flex-col gap-2">
          <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
            <Stat label="Выручка" value={formatSom(data.totals.total_tyiyn)} />
            <Stat label="Чеков" value={String(data.totals.count)} />
            <Stat label="Средний чек" value={data.totals.count ? formatSom(divRound(sold, data.totals.count)) : '—'} />
            <Stat
              label="Возвраты"
              value={returns.length ? `${returns.length} · ${formatSom(Math.abs(returnsSum))}` : '—'}
              tone={returns.length ? 'text-rose-700' : ''}
            />
          </div>
          {payments.length > 0 && (
            <div className="text-sm text-slate-600">
              {payments.map(([label, v], i) => (
                <span key={label}>
                  {i > 0 && ' · '}
                  {label} <Money value={v} className="font-medium text-slate-800" />
                </span>
              ))}
            </div>
          )}
        </div>
      )}
      <Card>
        {loading && !data ? (
          <Loading />
        ) : !data || data.sales.length === 0 ? (
          <Empty icon="receipt">За этот день чеков нет. Переключите день стрелками или откройте кассу.</Empty>
        ) : (
          <>
            {/* Телефон: компактная карточка — номер и время слева, сумма справа, ниже клиент и мастер. */}
            <ul className="-my-2 divide-y divide-slate-100 md:hidden">
              {pager.shown.map((s) => (
                <li key={s.id}>
                  <Link to={`/sales/${s.id}`} className="block py-2.5">
                    <div className="flex items-baseline justify-between gap-3">
                      <span className="font-medium">
                        № {s.number} <span className="font-normal text-slate-500">· {formatTime(s.created_at)}</span>
                      </span>
                      <Money value={s.total_tyiyn} tone="negative" className="font-semibold" />
                    </div>
                    <div className="mt-0.5 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-slate-500">
                      <span>
                        <Client name={s.party_name} phone={s.party_phone} fallback={s.sale_type === 'service' ? 'В сервис' : 'На вынос'} />
                      </span>
                      {s.master_name && <span>· мастер {s.master_name}</span>}
                      <Marks s={s} fix={fixes.data?.get(s.id)} />
                    </div>
                  </Link>
                </li>
              ))}
            </ul>
            <div className="hidden md:block">
              <Table head={['№', 'Время', 'Тип', 'Клиент', 'Кассир', 'Мастер', 'Сумма']}>
                {pager.shown.map((s) => (
                  <tr key={s.id} className="cursor-pointer hover:bg-slate-50" onClick={() => navigate(`/sales/${s.id}`)}>
                    <td className="px-2 py-2 font-medium">{s.number}</td>
                    <td className="px-2 py-2 tabular-nums">{formatTime(s.created_at)}</td>
                    <td className="px-2 py-2">
                      {s.kind === 'return' ? <Badge tone="rose">{s.no_receipt ? 'возврат без чека' : 'возврат'}</Badge> : s.sale_type === 'service' ? 'В сервис' : 'На вынос'}
                      <div>
                        <CorrectionBadges mark={fixes.data?.get(s.id)} />
                      </div>
                    </td>
                    <td className="px-2 py-2">
                      <Client name={s.party_name} phone={s.party_phone} fallback="—" />
                      {s.debt_tyiyn !== 0 && (
                        <div>
                          <Badge tone="amber">в долг {formatSom(Math.abs(s.debt_tyiyn))}</Badge>
                        </div>
                      )}
                    </td>
                    <td className="px-2 py-2">{s.cashier_name}</td>
                    <td className="px-2 py-2">{s.master_name ?? <span className="text-slate-400">—</span>}</td>
                    <td className="px-2 py-2 text-right">
                      <Money value={s.total_tyiyn} tone="negative" className="font-medium" />
                    </td>
                  </tr>
                ))}
              </Table>
            </div>
            <Pager page={pager.page} size={pager.size} total={pager.total} onPage={pager.setPage} onSize={pager.setSize} label="Страницы чеков" />
          </>
        )}
      </Card>
      {noReceipt && (
        <NoReceiptReturn
          onClose={() => {
            setNoReceipt(false)
            reload()
          }}
        />
      )}
    </div>
  )
}
