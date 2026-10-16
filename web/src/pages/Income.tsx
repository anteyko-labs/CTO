// «Куда сколько поступило» (SPEC-27 §6): оплаты чеков по способам, деньги по кассам, выручка по
// категориям и товарам, по каждой накладной — продано, осталось, заработано (оценка).
// Администратору сервер не отдаёт сейф владельца и накладные (там закупка и заработок).
import { Fragment, useState } from 'react'
import { Link } from 'react-router-dom'
import { PeriodPicker } from '../components/PeriodPicker'
import { stockText } from '../components/ProductPicker'
import { Card, CardTitle, Empty, ErrorBox, Loading, Money, Notice, PageHeader, Table } from '../components/ui'
import { get, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatDate, formatDateTime, formatSom } from '../lib/format'
import { useLoad, useRangeParam } from '../lib/hooks'
import { METHOD_LABELS, MOVEMENT_LABELS } from '../lib/income'
import type { IncomeReport } from '../lib/types'

const qtyText = (unit: 'piece' | 'ml' | 'g', containerMl: number | null, qty: number) =>
  stockText({ unit, container_ml: containerMl, stock_qty: qty })

export default function Income() {
  const owner = useUser().role === 'owner'
  const [range, today, setRange] = useRangeParam()
  const [openAccount, setOpenAccount] = useState<string | null>(null)
  const [openReceipt, setOpenReceipt] = useState<string | null>(null)
  const [allProducts, setAllProducts] = useState(false)
  const r = useLoad(() => get<IncomeReport>(`/reports/income${qs({ from: range.from, to: range.to })}`), [range.from, range.to])
  const data = r.data
  const subtitle = range.from === range.to ? `за ${formatDate(range.from)}` : `с ${formatDate(range.from)} по ${formatDate(range.to)}`
  const methods = (data?.methods ?? []).filter((m) => m.sales_tyiyn !== 0 || m.returns_tyiyn !== 0)
  const products = data?.products ?? []

  return (
    <div>
      <PageHeader title="Куда сколько поступило" subtitle={subtitle} />
      <PeriodPicker value={range} today={today} onChange={setRange} />
      <ErrorBox error={r.error} />
      {r.loading && !data ? (
        <Loading />
      ) : data ? (
        <div className="flex flex-col gap-4">
          <Card>
            <CardTitle>Оплаты чеков по способам</CardTitle>
            {methods.length === 0 ? (
              <Empty>Чеков за период не было</Empty>
            ) : (
              <Table head={['Способ', 'Продажи', 'Возвраты', 'Итого', 'Комиссия банка']}>
                {methods.map((m) => (
                  <tr key={m.method} data-testid="income-method">
                    <td className="px-2 py-2 font-medium">{METHOD_LABELS[m.method] ?? m.method}</td>
                    <td className="px-2 py-2 text-right">
                      <Money value={m.sales_tyiyn} />
                    </td>
                    <td className="px-2 py-2 text-right">
                      <Money value={m.returns_tyiyn} zero="—" />
                    </td>
                    <td className="px-2 py-2 text-right font-semibold">
                      <Money value={m.net_tyiyn} tone="negative" />
                    </td>
                    <td className="px-2 py-2 text-right">
                      <Money value={m.fee_tyiyn} zero="—" />
                    </td>
                  </tr>
                ))}
                <tr className="border-t-2 border-slate-200 font-semibold" data-testid="income-total">
                  <td className="px-2 py-2">Выручка</td>
                  <td className="px-2 py-2 text-right">
                    <Money value={data.sales_tyiyn} />
                  </td>
                  <td className="px-2 py-2 text-right">
                    <Money value={data.returns_tyiyn} zero="—" />
                  </td>
                  <td className="px-2 py-2 text-right">
                    <Money value={data.revenue_tyiyn} tone="negative" />
                  </td>
                  <td className="px-2 py-2 text-right">
                    <Money value={data.fee_tyiyn} zero="—" />
                  </td>
                </tr>
              </Table>
            )}
            <div className="mt-2 text-xs text-slate-500">
              Товары {formatSom(data.goods_tyiyn)}, работы {formatSom(data.services_tyiyn)}. «В долг», «Баллы» и «Аккумулятор» — оплаты без
              денег; погашения долгов — в кассах ниже.
            </div>
          </Card>

          <Card>
            <CardTitle>Деньги по кассам</CardTitle>
            <Table head={['Касса', 'На начало', 'Пришло', 'Ушло', 'На конец']}>
              {data.accounts.map((a) => (
                <Fragment key={a.account_id}>
                  <tr className="cursor-pointer hover:bg-slate-50" onClick={() => setOpenAccount(openAccount === a.account_id ? null : a.account_id)}>
                    <td className="px-2 py-2 font-medium">
                      {a.name} <span className="text-xs text-slate-400">{openAccount === a.account_id ? '▲' : '▼'}</span>
                    </td>
                    <td className="px-2 py-2 text-right">
                      <Money value={a.opening_tyiyn} tone="negative" />
                    </td>
                    <td className="px-2 py-2 text-right text-emerald-700">
                      <Money value={a.in_tyiyn} zero="—" />
                    </td>
                    <td className="px-2 py-2 text-right text-rose-700">
                      <Money value={a.out_tyiyn} zero="—" />
                    </td>
                    <td className="px-2 py-2 text-right font-semibold">
                      <Money value={a.closing_tyiyn} tone="negative" />
                    </td>
                  </tr>
                  {openAccount === a.account_id &&
                    a.kinds.map((k) => (
                      <tr key={`${a.account_id}:${k.kind}`} className="text-xs text-slate-600">
                        <td className="px-2 py-1 pl-6">{MOVEMENT_LABELS[k.kind] ?? k.kind}</td>
                        <td className="px-2 py-1" />
                        <td className="px-2 py-1 text-right">
                          <Money value={k.in_tyiyn} zero="—" />
                        </td>
                        <td className="px-2 py-1 text-right">
                          <Money value={k.out_tyiyn} zero="—" />
                        </td>
                        <td className="px-2 py-1" />
                      </tr>
                    ))}
                </Fragment>
              ))}
            </Table>
            {owner && (
              <div className="mt-2 text-xs text-slate-500">
                Безнал подробно — на экране{' '}
                <Link className="text-sky-700 underline" to={`/bank${qs({ from: range.from, to: range.to })}`}>
                  «Счёт»
                </Link>
                .
              </div>
            )}
          </Card>

          <div className="grid gap-4 lg:grid-cols-2">
            <Card>
              <CardTitle>Выручка по категориям</CardTitle>
              {data.categories.length === 0 && data.services_tyiyn === 0 ? (
                <Empty>Продаж за период нет</Empty>
              ) : (
                <Table head={['Категория', 'Выручка']}>
                  {data.categories.map((c) => (
                    <tr key={c.name}>
                      <td className="px-2 py-2">{c.name}</td>
                      <td className="px-2 py-2 text-right">
                        <Money value={c.revenue_tyiyn} tone="negative" />
                      </td>
                    </tr>
                  ))}
                  {data.services_tyiyn !== 0 && (
                    <tr>
                      <td className="px-2 py-2">Работы (услуги)</td>
                      <td className="px-2 py-2 text-right">
                        <Money value={data.services_tyiyn} tone="negative" />
                      </td>
                    </tr>
                  )}
                </Table>
              )}
            </Card>
            <Card>
              <CardTitle>Выручка по товарам</CardTitle>
              {products.length === 0 ? (
                <Empty>Продаж товаров нет</Empty>
              ) : (
                <>
                  <Table head={['Товар', 'Продано', 'Выручка']}>
                    {(allProducts ? products : products.slice(0, 15)).map((p) => (
                      <tr key={p.product_id}>
                        <td className="px-2 py-2">{p.name}</td>
                        <td className="whitespace-nowrap px-2 py-2 tabular-nums">{qtyText(p.unit, p.container_ml, p.qty)}</td>
                        <td className="px-2 py-2 text-right">
                          <Money value={p.amount_tyiyn} />
                        </td>
                      </tr>
                    ))}
                  </Table>
                  {products.length > 15 && (
                    <button type="button" className="mt-1 min-h-[42px] text-sm text-sky-700 underline" onClick={() => setAllProducts(!allProducts)}>
                      {allProducts ? 'Свернуть' : `Все (${products.length})`}
                    </button>
                  )}
                </>
              )}
            </Card>
          </div>

          {data.receipts && (
            <Card>
              <CardTitle>По накладным: продано, осталось, заработано</CardTitle>
              <Notice tone="sky">
                Оценка: склад считается по средней, без партий, поэтому остаток раскладывается по накладным «первым продаётся пришедшее
                раньше». Продано и осталось — на сейчас, по закупке; выручка — по средней цене продаж товара после прихода.
              </Notice>
              {data.receipts.length === 0 ? (
                <Empty>Накладных за период нет</Empty>
              ) : (
                <div className="mt-3">
                  <Table head={['Накладная', 'Закупка', 'Продано (по закупке)', 'Осталось (по закупке)', 'Выручка', 'Заработано']}>
                    {data.receipts.map((x) => (
                      <Fragment key={x.receipt_id}>
                        <tr className="cursor-pointer hover:bg-slate-50" onClick={() => setOpenReceipt(openReceipt === x.receipt_id ? null : x.receipt_id)}>
                          <td className="px-2 py-2">
                            <div className="font-medium">
                              № {x.number} <span className="text-xs text-slate-400">{openReceipt === x.receipt_id ? '▲' : '▼'}</span>
                            </div>
                            <div className="text-xs text-slate-500">
                              {formatDateTime(x.created_at)}
                              {x.supplier_name && ` · ${x.supplier_name}`}
                            </div>
                          </td>
                          <td className="px-2 py-2 text-right">
                            <Money value={x.cost_tyiyn} />
                          </td>
                          <td className="px-2 py-2 text-right">
                            <Money value={x.sold_cost_tyiyn} />
                          </td>
                          <td className="px-2 py-2 text-right">
                            <Money value={x.left_cost_tyiyn} zero="—" />
                          </td>
                          <td className="px-2 py-2 text-right">
                            <Money value={x.revenue_tyiyn} />
                          </td>
                          <td className="px-2 py-2 text-right font-semibold">
                            <Money value={x.earned_tyiyn} tone="sign" />
                          </td>
                        </tr>
                        {openReceipt === x.receipt_id &&
                          x.items.map((i) => (
                            <tr key={`${x.receipt_id}:${i.product_id}`} className="text-xs text-slate-600">
                              <td className="px-2 py-1 pl-6">
                                {i.name}: привезли {qtyText(i.unit, i.container_ml, i.qty)}, продано {qtyText(i.unit, i.container_ml, i.sold_qty)}, осталось{' '}
                                {qtyText(i.unit, i.container_ml, i.left_qty)}
                              </td>
                              <td className="px-2 py-1 text-right">
                                <Money value={i.cost_tyiyn} />
                              </td>
                              <td className="px-2 py-1 text-right">
                                <Money value={i.sold_cost_tyiyn} />
                              </td>
                              <td className="px-2 py-1 text-right">
                                <Money value={i.left_cost_tyiyn} zero="—" />
                              </td>
                              <td className="px-2 py-1 text-right">
                                <Money value={i.revenue_tyiyn} />
                              </td>
                              <td className="px-2 py-1 text-right">
                                <Money value={i.earned_tyiyn} tone="sign" />
                              </td>
                            </tr>
                          ))}
                      </Fragment>
                    ))}
                  </Table>
                </div>
              )}
            </Card>
          )}
        </div>
      ) : null}
    </div>
  )
}
