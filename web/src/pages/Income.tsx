// «Куда сколько поступило» (SPEC-27 §6): оплаты чеков по способам, деньги по кассам, выручка по
// категориям и товарам, по каждой накладной — продано, осталось, заработано (оценка).
// Администратору сервер не отдаёт сейф владельца и накладные (там закупка и заработок).
import { Fragment, useState } from 'react'
import { Link } from 'react-router-dom'
import { Pager, usePager } from '../components/Pager'
import { PeriodPicker } from '../components/PeriodPicker'
import { ExportButton } from '../components/ExportButton'
import { exportTitle, periodText, unitName, xl } from '../lib/exportTable'
import type { Sheet, SheetRow } from '../lib/xlsxWrite'
import { stockText } from '../components/ProductPicker'
import { Card, CardTitle, Empty, ErrorBox, Loading, Money, Notice, PageHeader, Table } from '../components/ui'
import { get, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatDate, formatDateTime, formatSom } from '../lib/format'
import { useLoad, useRangeParam } from '../lib/hooks'
import { METHOD_LABELS, MOVEMENT_LABELS } from '../lib/income'
import type { IncomeReport } from '../lib/types'
import { BranchBreakdown, BranchScope } from '../components/BranchScope'
import { branchParam, useAllBranches, type AllBranches } from '../lib/branchReport'

const qtyText = (unit: 'piece' | 'ml' | 'g', containerMl: number | null, qty: number) =>
  stockText({ unit, container_ml: containerMl, stock_qty: qty })

export default function Income() {
  const user = useUser()
  const owner = user.role === 'owner'
  // «Все филиалы» (SPEC-33 §3).
  const [all, setAll] = useAllBranches(owner && Boolean(user.several_branches))
  const [range, today, setRange] = useRangeParam()
  const [openAccount, setOpenAccount] = useState<string | null>(null)
  const [openReceipt, setOpenReceipt] = useState<string | null>(null)
  const [allProducts, setAllProducts] = useState(false)
  const r = useLoad(
    () => get<AllBranches<IncomeReport>>(`/reports/income${qs({ from: range.from, to: range.to, ...branchParam(all) })}`),
    [range.from, range.to, all],
  )
  const data = r.data
  const subtitle = range.from === range.to ? `за ${formatDate(range.from)}` : `с ${formatDate(range.from)} по ${formatDate(range.to)}`
  const methods = (data?.methods ?? []).filter((m) => m.sales_tyiyn !== 0 || m.returns_tyiyn !== 0)
  const products = data?.products ?? []
  // Накладные периода — по страницам; другой период — первая страница.
  const receiptsPager = usePager(data?.receipts ?? [], 'income.receipts', `${range.from}:${range.to}`)

  return (
    <div>
      <PageHeader
        title="Куда сколько поступило"
        subtitle={subtitle}
        actions={
          // Все таблицы экрана листами одного файла; накладные с закупкой — только если их отдал сервер (I13).
          <ExportButton
            file="Куда сколько поступило"
            period={periodText(range.from, range.to)}
            disabled={!data}
            sheets={() => (data ? incomeSheets(data, [`период ${periodText(range.from, range.to)}`, all && 'все филиалы']) : [])}
          />
        }
      />
      <PeriodPicker value={range} today={today} onChange={setRange} />
      <BranchScope user={user} all={all} onChange={setAll} />
      <ErrorBox error={r.error} />
      {r.loading && !data ? (
        <Loading />
      ) : data ? (
        <div className="flex flex-col gap-4">
          <BranchBreakdown
            parts={data.branches}
            columns={[
              { label: 'Продажи', value: (x) => x.sales_tyiyn },
              { label: 'Возвраты', value: (x) => x.returns_tyiyn },
              { label: 'Выручка', value: (x) => x.revenue_tyiyn },
            ]}
          />
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
                    {receiptsPager.shown.map((x) => (
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
                  <Pager
                    page={receiptsPager.page}
                    size={receiptsPager.size}
                    total={receiptsPager.total}
                    onPage={receiptsPager.setPage}
                    onSize={receiptsPager.setSize}
                    label="Страницы накладных периода"
                  />
                </div>
              )}
            </Card>
          )}
        </div>
      ) : null}
    </div>
  )
}

/** «Куда сколько поступило» в Excel: способы оплаты, кассы с видами движений, категории, все товары и — владельцу — накладные. */
function incomeSheets(d: IncomeReport, filter: (string | false)[]): Sheet[] {
  const title = (part: string) => exportTitle(`Куда сколько поступило — ${part}`, filter)
  const money = xl.money
  const accounts: SheetRow[] = d.accounts.flatMap((a) => [
    { cells: [a.name, money(a.opening_tyiyn), money(a.in_tyiyn), money(a.out_tyiyn), money(a.closing_tyiyn)], bold: true },
    ...a.kinds.map((k) => ({ cells: [MOVEMENT_LABELS[k.kind] ?? k.kind, null, money(k.in_tyiyn), money(k.out_tyiyn)], level: 1 })),
  ])
  const sheets: Sheet[] = [
    {
      name: 'Способы оплаты',
      columns: ['Способ', 'Продажи', 'Возвраты', 'Итого', 'Комиссия банка'].map((label) => ({ label })),
      title: title('оплаты чеков по способам'),
      rows: [
        ...d.methods
          .filter((m) => m.sales_tyiyn !== 0 || m.returns_tyiyn !== 0)
          .map((m) => [METHOD_LABELS[m.method] ?? m.method, money(m.sales_tyiyn), money(m.returns_tyiyn), money(m.net_tyiyn), money(m.fee_tyiyn)]),
        { cells: ['Выручка', money(d.sales_tyiyn), money(d.returns_tyiyn), money(d.revenue_tyiyn), money(d.fee_tyiyn)], bold: true },
        ['в т. ч. товары', null, null, money(d.goods_tyiyn)],
        ['в т. ч. работы', null, null, money(d.services_tyiyn)],
      ],
    },
    { name: 'Кассы', columns: ['Касса', 'На начало', 'Пришло', 'Ушло', 'На конец'].map((label) => ({ label })), title: title('деньги по кассам'), rows: accounts, filter: false },
    {
      name: 'Категории',
      columns: [{ label: 'Категория' }, { label: 'Выручка' }],
      title: title('выручка по категориям'),
      rows: [...d.categories.map((c) => [c.name, money(c.revenue_tyiyn)]), ...(d.services_tyiyn !== 0 ? [['Работы (услуги)', money(d.services_tyiyn)]] : [])],
    },
    {
      name: 'Товары',
      columns: ['Товар', 'Продано', 'Ед.', 'Выручка'].map((label) => ({ label })),
      title: title('выручка по товарам'),
      rows: d.products.map((p) => [p.name, xl.units(p.qty, p.unit), unitName(p.unit), money(p.amount_tyiyn)]),
    },
  ]
  // Накладные с закупкой и заработком сервер отдаёт только владельцу (ADR-008, I13).
  if (d.receipts) {
    sheets.push({
      name: 'Накладные',
      columns: ['Накладная', 'Дата', 'Поставщик', 'Товар', 'Привезли', 'Продано', 'Осталось', 'Ед.', 'Закупка', 'Продано (по закупке)', 'Осталось (по закупке)', 'Выручка', 'Заработано'].map((label) => ({ label })),
      title: title('по накладным: продано, осталось, заработано (оценка)'),
      filter: false,
      rows: d.receipts.flatMap((x) => [
        {
          cells: [`№ ${x.number}`, xl.dateTime(x.created_at), x.supplier_name ?? '', null, null, null, null, null, money(x.cost_tyiyn), money(x.sold_cost_tyiyn), money(x.left_cost_tyiyn), money(x.revenue_tyiyn), money(x.earned_tyiyn)],
          bold: true,
        },
        ...x.items.map((i) => ({
          cells: [
            `№ ${x.number}`,
            null,
            null,
            i.name,
            xl.units(i.qty, i.unit),
            xl.units(i.sold_qty, i.unit),
            xl.units(i.left_qty, i.unit),
            unitName(i.unit),
            money(i.cost_tyiyn),
            money(i.sold_cost_tyiyn),
            money(i.left_cost_tyiyn),
            money(i.revenue_tyiyn),
            money(i.earned_tyiyn),
          ],
          level: 1,
        })),
      ]),
    })
  }
  return sheets
}
