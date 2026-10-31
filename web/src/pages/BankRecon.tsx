// «Безнал (банк)» → «Сверка с банком» (SPEC-31 §3): по дням — у нас, в банке, расхождение, комиссия
// банка и поправка; несовпавшие оплаты с обеих сторон. «Сверить с банком» берёт выписку за
// выбранные дни (не больше 14). Только владелец; считает сервер.
import { Link } from 'react-router-dom'
import { PeriodPicker } from '../components/PeriodPicker'
import { ExportButton } from '../components/ExportButton'
import { listSheet, periodText, xl, type ExportCol } from '../lib/exportTable'
import { Badge, Button, Card, CardTitle, Empty, ErrorBox, Loading, Money, Notice, PageHeader, Table, toast } from '../components/ui'
import { get, post, qs } from '../lib/api'
import { formatDate, formatDateTime, formatSom } from '../lib/format'
import { useAction, useLoad, useRangeParam } from '../lib/hooks'
import { periodDays, RECON_KIND, type ReconBankItem, type ReconDay, type ReconOurItem, type ReconOut } from '../lib/optima'

export default function BankRecon() {
  const [range, today, setRange] = useRangeParam()
  const r = useLoad(() => get<ReconOut>(`/bank/reconcile${qs({ from: range.from, to: range.to })}`), [range.from, range.to])
  const act = useAction()
  const data = r.data
  const days = periodDays(range.from, range.to)
  const tooLong = days > 14
  return (
    <div>
      <PageHeader
        title="Сверка с банком"
        subtitle={`Optima Business${data?.account_name ? ` · счёт «${data.account_name}»` : ''}: оплаты картой и по QR в учёте и в выписке банка`}
        back={
          <Link to="/bank" className="text-sm text-sky-700 hover:underline">
            ← Безнал (банк)
          </Link>
        }
        actions={
          // Три таблицы сверки — листами одного файла (задача 003 Н13).
          <ExportButton
            file="Сверка с банком"
            period={periodText(range.from, range.to)}
            disabled={!data}
            sheets={() => {
              if (!data) return []
              const filter = [`период ${periodText(range.from, range.to)}`, data.account_name && `счёт «${data.account_name}»`]
              return [
                listSheet('Сверка с банком по дням', DAY_COLS, data.days, { sheet: 'По дням', filter }),
                listSheet('В учёте, нет в банке', OURS_COLS, data.ours_unmatched, { sheet: 'Нет в банке', filter }),
                listSheet('В банке, нет в учёте', BANK_COLS, data.bank_unmatched, { sheet: 'Нет в учёте', filter }),
              ]
            }}
          />
        }
      />
      <PeriodPicker value={range} today={today} onChange={setRange} />
      <ErrorBox error={r.error} />
      {data && !data.enabled && <Notice>Ключ Optima Business не задан на сервере — выписку не загрузить.</Notice>}
      {data && data.enabled && !data.configured && (
        <Notice>
          Укажите расчётный счёт в <Link to="/settings/optima" className="underline">«Настройки» → «Optima Business»</Link>.
        </Notice>
      )}
      <div className="mb-4 flex flex-wrap items-center gap-3">
        <Button
          disabled={!data?.enabled || !data.configured || act.busy || tooLong || range.to > today}
          onClick={() =>
            void act.run(async () => {
              const out = await post<ReconOut>('/bank/reconcile', { from: range.from, to: range.to })
              r.setData(out)
              const run = out.run
              toast(
                run
                  ? `Выписка: операций ${run.operations}, новых ${run.new_operations}${run.adjustments ? `; поправка комиссии ${formatSom(run.adjusted_tyiyn)}` : ''}`
                  : 'Сверено',
              )
            })
          }
        >
          {act.busy ? 'Сверяем…' : 'Сверить с банком'}
        </Button>
        <span className="text-sm text-slate-500">
          {tooLong ? 'Выписка банка — не больше 14 дней за раз: сократите период' : 'Сама — каждый день в 06:00 за три прошедших дня'}
        </span>
      </div>
      <ErrorBox error={act.error} />
      {r.loading && !data ? (
        <Loading />
      ) : data ? (
        <div className="flex flex-col gap-4">
          <Card>
            <CardTitle>По дням</CardTitle>
            <Table head={['День', 'У нас', 'В банке', 'Расхождение', 'Комиссия банка', 'Учтено', 'Поправка', '']}>
              {data.days.map((d) => (
                <tr key={d.date} data-testid="recon-day">
                  <td className="px-2 py-2 whitespace-nowrap">{formatDate(d.date)}</td>
                  <td className="px-2 py-2 text-right tabular-nums">
                    {d.ours_count} · <Money value={d.ours_tyiyn} />
                  </td>
                  <td className="px-2 py-2 text-right tabular-nums">{d.loaded ? <>{d.bank_count} · <Money value={d.bank_tyiyn} /></> : '—'}</td>
                  <td className="px-2 py-2 text-right">{d.loaded ? <Money value={d.diff_tyiyn} tone="sign" zero="—" /> : '—'}</td>
                  <td className="px-2 py-2 text-right">{d.loaded ? <Money value={d.bank_fee_tyiyn} zero="—" /> : '—'}</td>
                  <td className="px-2 py-2 text-right">
                    <Money value={d.our_fee_tyiyn} zero="—" />
                  </td>
                  <td className="px-2 py-2 text-right">
                    <Money value={d.adjusted_tyiyn} zero="—" />
                  </td>
                  <td className="px-2 py-2">
                    {!d.loaded ? (
                      <Badge>выписки нет</Badge>
                    ) : d.unmatched > 0 ? (
                      <Badge tone="rose">не совпало: {d.unmatched}</Badge>
                    ) : d.ours_count + d.bank_count > 0 ? (
                      <Badge tone="green">сошлось</Badge>
                    ) : null}
                  </td>
                </tr>
              ))}
            </Table>
          </Card>
          <Card>
            <CardTitle>В учёте, нет в банке: {data.ours_unmatched.length}</CardTitle>
            {data.ours_unmatched.length === 0 ? (
              <Empty>Всё, что в учёте, банк подтвердил</Empty>
            ) : (
              <Table head={['Когда', 'Что', 'Сумма']}>
                {data.ours_unmatched.map((o, i) => (
                  <tr key={`${o.at}-${i}`} data-testid="recon-ours">
                    <td className="px-2 py-2 whitespace-nowrap text-slate-500">{formatDateTime(o.at)}</td>
                    <td className="px-2 py-2">
                      {o.sale_id ? (
                        <Link to={`/sales/${o.sale_id}`} className="text-sky-700 hover:underline">
                          чек № {o.sale_number}
                        </Link>
                      ) : null}{' '}
                      <span className="text-slate-500">({RECON_KIND[o.kind]})</span>
                    </td>
                    <td className="px-2 py-2 text-right">
                      <Money value={o.amount_tyiyn} />
                    </td>
                  </tr>
                ))}
              </Table>
            )}
          </Card>
          <Card>
            <CardTitle>В банке, нет в учёте: {data.bank_unmatched.length}</CardTitle>
            {data.bank_unmatched.length === 0 ? (
              <Empty>Все операции банка есть в учёте</Empty>
            ) : (
              <Table head={['Когда', 'Что', 'Комиссия', 'Сумма']}>
                {data.bank_unmatched.map((b) => (
                  <tr key={b.id} data-testid="recon-bank">
                    <td className="px-2 py-2 whitespace-nowrap text-slate-500">{formatDateTime(b.at)}</td>
                    <td className="px-2 py-2">
                      {RECON_KIND[b.kind]}
                      <span className="text-slate-500">
                        {b.requisite ? ` · ${b.requisite}` : ''}
                        {b.auth_code ? ` · код ${b.auth_code}` : ''}
                        {b.rrn ? ` · RRN ${b.rrn}` : ''}
                      </span>
                    </td>
                    <td className="px-2 py-2 text-right">
                      <Money value={b.fee_tyiyn} zero="—" />
                    </td>
                    <td className="px-2 py-2 text-right">
                      <Money value={b.amount_tyiyn} />
                    </td>
                  </tr>
                ))}
              </Table>
            )}
          </Card>
          <div className="text-sm text-slate-500">
            Карта сопоставляется по сумме и времени (±15 минут), QR — по номеру операции банка. Комиссия правится только за дни, где сошлись все оплаты:
            разница между комиссией банка и учтённой по ставке счёта ложится движением «комиссия банка» и входит в прибыль.
          </div>
        </div>
      ) : null}
    </div>
  )
}

const DAY_COLS: ExportCol<ReconDay>[] = [
  { label: 'День', value: (d) => xl.date(d.date) },
  { label: 'У нас, оплат', value: (d) => xl.int(d.ours_count) },
  { label: 'У нас, сумма', value: (d) => xl.money(d.ours_tyiyn) },
  { label: 'В банке, оплат', value: (d) => (d.loaded ? xl.int(d.bank_count) : null) },
  { label: 'В банке, сумма', value: (d) => (d.loaded ? xl.money(d.bank_tyiyn) : null) },
  { label: 'Расхождение', value: (d) => (d.loaded ? xl.money(d.diff_tyiyn) : null) },
  { label: 'Комиссия банка', value: (d) => (d.loaded ? xl.money(d.bank_fee_tyiyn) : null) },
  { label: 'Учтено', value: (d) => xl.money(d.our_fee_tyiyn) },
  { label: 'Поправка', value: (d) => xl.money(d.adjusted_tyiyn) },
  { label: 'Итог', value: (d) => (!d.loaded ? 'выписки нет' : d.unmatched > 0 ? `не совпало: ${d.unmatched}` : d.ours_count + d.bank_count > 0 ? 'сошлось' : '') },
]

const OURS_COLS: ExportCol<ReconOurItem>[] = [
  { label: 'Когда', value: (o) => xl.dateTime(o.at) },
  { label: 'Чек', value: (o) => (o.sale_number != null ? xl.int(o.sale_number) : null) },
  { label: 'Что', value: (o) => RECON_KIND[o.kind] },
  { label: 'Сумма', value: (o) => xl.money(o.amount_tyiyn) },
]

const BANK_COLS: ExportCol<ReconBankItem>[] = [
  { label: 'Когда', value: (b) => xl.dateTime(b.at) },
  { label: 'Что', value: (b) => RECON_KIND[b.kind] },
  { label: 'Реквизит', value: (b) => b.requisite ?? '' },
  { label: 'Код', value: (b) => b.auth_code ?? '' },
  { label: 'RRN', value: (b) => b.rrn ?? '' },
  { label: 'Комиссия', value: (b) => xl.money(b.fee_tyiyn) },
  { label: 'Сумма', value: (b) => xl.money(b.amount_tyiyn) },
]
