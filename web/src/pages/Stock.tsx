// «Склад → Остатки» (SPEC-36 §14, ADR-125): что фактически лежит на складе. Только ненулевые остатки,
// минус — красным с пометкой «проверить»; группы по категориям с подытогами (позиции, количество,
// суммы по цене продажи, владельцу — и по закупке, ADR-008), внизу итог склада. Подытоги и суммы
// считает сервер (`GET /products/list?stock=in_stock&group=category`). «Товары» остаются справочником.
import { useState } from 'react'
import { Navigate, useLocation, useSearchParams } from 'react-router-dom'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, PageHeader, Table } from '../components/ui'
import { get, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatSom } from '../lib/format'
import { useDebounced, useLoad } from '../lib/hooks'
import { printInFrame } from '../lib/printFrame'
import { isOldStockLink, stockToProducts } from '../lib/stockLink'
import { groupQty, groupRows, pricePer, rowQty, rowViscosity, stockSheetHtml, stockTotal } from '../lib/stockSheet'
import type { Category, ProductList } from '../lib/types'
import { qtyText } from '../lib/units'

/** Сколько строк берём за раз: склад точки — сотни позиций; подытоги — по всему отбору. */
const LIMIT = 2000

export default function Stock() {
  // Старые ссылки «Остатков» с отбором (SPEC-30 §11) — в «Товары», как раньше.
  const { search } = useLocation()
  if (isOldStockLink(search)) return <Navigate to={stockToProducts(search)} replace />
  return <StockPage />
}

function StockPage() {
  const owner = useUser().role === 'owner'
  const [params] = useSearchParams()
  const [q, setQ] = useState(() => params.get('q') ?? '')
  const [categoryId, setCategoryId] = useState(() => params.get('category_id') ?? '')
  const query = useDebounced(q.trim(), 300)
  const categories = useLoad(() => get<Category[]>('/categories'), [])
  const list = useLoad(
    () => get<ProductList>(`/products/list${qs({ stock: 'in_stock', group: 'category', limit: LIMIT, q: query, category_id: categoryId })}`),
    [query, categoryId],
  )
  const totals = list.data?.totals
  const groups = list.data ? groupRows(list.data.items, totals?.category_totals ?? []) : []
  const shown = list.data?.items.length ?? 0
  const category = categories.data?.find((c) => c.id === categoryId)

  const print = () => {
    if (!totals) return
    const filter = [category && `категория «${category.name}»`, query && `поиск «${query}»`].filter(Boolean).join(', ')
    printInFrame(stockSheetHtml({ at: new Date().toISOString(), groups, total: totals, filter }))
  }

  const money = (v: number | undefined) => (v === undefined ? '' : formatSom(v))
  const cols = owner ? 7 : 6

  return (
    <div>
      <PageHeader
        title="Остатки"
        subtitle="Что лежит на складе: только товары с остатком, по категориям. Справочник всех товаров — «Товары»."
        actions={
          <Button variant="secondary" onClick={print} disabled={!totals || totals.count === 0} data-testid="stock-print">
            Печать
          </Button>
        }
      />
      <Card className="mb-4">
        <div className="grid gap-3 sm:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]">
          <Field label="Поиск">
            <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Название, артикул или штрихкод" data-testid="stock-search" />
          </Field>
          <Field label="Категория">
            <select value={categoryId} onChange={(e) => setCategoryId(e.target.value)} data-testid="stock-category">
              <option value="">Все категории</option>
              {(categories.data ?? []).map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name}
                </option>
              ))}
            </select>
          </Field>
        </div>
      </Card>
      <ErrorBox error={list.error} />
      {!list.data ? (
        !list.error && <Loading />
      ) : groups.length === 0 ? (
        <Card>
          <Empty icon="stock">{query || categoryId ? 'По отбору товаров с остатком нет.' : 'На складе пусто: товаров с остатком нет.'}</Empty>
        </Card>
      ) : (
        <Card className="p-0" data-testid="stock-table">
          {totals && shown < totals.count && (
            <div className="px-4 pt-3 text-sm text-amber-800">
              Показаны первые {shown} позиций из {totals.count} — уточните поиск. Подытоги — по всем.
            </div>
          )}
          <Table
            head={['Товар', 'Вязкость', 'Остаток', 'Минимум', 'Цена', 'Сумма (продажа)', ...(owner ? ['Сумма (закупка)'] : [])]}
          >
            {groups.map((g) => [
              <tr key={`h-${g.total.category_id}`} className="bg-slate-50" data-testid="stock-group">
                <td colSpan={cols} className="px-2 py-2 font-semibold">
                  {g.total.name}
                </td>
              </tr>,
              ...g.rows.map((r) => {
                const v = rowViscosity(r)
                const minus = r.stock_qty < 0
                return (
                  <tr key={r.id} data-testid="stock-row" className={minus ? 'text-rose-700' : ''}>
                    <td className="px-2 py-2">
                      {r.name}
                      {minus && (
                        <span className="ml-2">
                          <Badge tone="rose">проверить</Badge>
                        </span>
                      )}
                    </td>
                    <td className="px-2 py-2">
                      {v && (
                        <span className={v.fromName ? 'text-slate-400' : ''} title={v.fromName ? 'Из названия — заполните в карточке' : undefined}>
                          {v.text}
                        </span>
                      )}
                    </td>
                    <td className={`px-2 py-2 text-right tabular-nums ${minus ? 'font-semibold' : ''}`}>{rowQty(r)}</td>
                    <td className="px-2 py-2 text-right tabular-nums text-slate-500">{r.min_stock > 0 ? qtyText(r.unit, r.min_stock) : ''}</td>
                    <td className="px-2 py-2 text-right tabular-nums">
                      {formatSom(r.sale_price_tyiyn)} <span className="text-xs text-slate-500">{pricePer(r)}</span>
                    </td>
                    <td className="px-2 py-2 text-right tabular-nums">{formatSom(r.stock_sale_tyiyn)}</td>
                    {owner && <td className="px-2 py-2 text-right tabular-nums">{money(r.stock_purchase_tyiyn)}</td>}
                  </tr>
                )
              }),
              <tr key={`t-${g.total.category_id}`} className="font-semibold" data-testid="stock-subtotal">
                <td className="px-2 py-2">
                  Итого: {g.total.count} поз.
                </td>
                <td />
                <td className="px-2 py-2 text-right tabular-nums">{groupQty(g.total)}</td>
                <td />
                <td />
                <td className="px-2 py-2 text-right tabular-nums">{formatSom(g.total.sale_tyiyn)}</td>
                {owner && <td className="px-2 py-2 text-right tabular-nums">{money(g.total.purchase_tyiyn)}</td>}
              </tr>,
            ])}
            {totals && (
              <tr className="border-t-2 border-slate-300 font-bold" data-testid="stock-total">
                <td className="px-2 py-2">Итого по складу: {totals.count} поз.</td>
                <td />
                <td className="px-2 py-2 text-right tabular-nums">{groupQty(stockTotal(totals.category_totals ?? []))}</td>
                <td />
                <td />
                <td className="px-2 py-2 text-right tabular-nums">{formatSom(totals.sale_tyiyn)}</td>
                {owner && <td className="px-2 py-2 text-right tabular-nums">{money(totals.purchase_tyiyn)}</td>}
              </tr>
            )}
          </Table>
        </Card>
      )}
    </div>
  )
}
