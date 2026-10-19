// «Оприходование» в меню «Склад» (SPEC-29 §25, §8): все оприходования без поставщика за период со
// страницами и кнопка «+ Оприходовать» — выбор товара, потом то же окно, что у товара на экране
// «Товары». Цену закупки видят владелец и администратор (ADR-008); права проверяет сервер.
import { useState } from 'react'
import { CorrectionBadges } from '../components/Correction'
import { Icon } from '../components/icons'
import { Pager, usePagedLoad } from '../components/Pager'
import { ProductPicker } from '../components/ProductPicker'
import { StockInModal } from '../components/StockInModal'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, Modal, PageHeader, Table } from '../components/ui'
import { get, qs } from '../lib/api'
import { correctionMarks, loadCorrections } from '../lib/corrections'
import { formatDateTime, formatKg, formatLiters, formatSom, shiftDate, todayBishkek } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import type { Product, StockIn } from '../lib/types'

const qtyText = (unit: string, q: number) => (unit === 'ml' ? formatLiters(q) : unit === 'g' ? formatKg(q) : `${q} шт`)

export default function StockIns() {
  const [to, setTo] = useState(todayBishkek)
  const [from, setFrom] = useState(() => shiftDate(todayBishkek(), -30))
  const list = usePagedLoad<StockIn>(`/stock-ins${qs({ from, to })}`, 'stock-ins')
  const marks = useLoad(() => loadCorrections('stock_in').then(correctionMarks), [from, to])
  // «+ Оприходовать»: сначала товар, потом окно оприходования (в нём же сторно и исправление).
  const [picking, setPicking] = useState(false)
  const [product, setProduct] = useState<Product | null>(null)
  const open = useAction()
  const openFor = (id: string) =>
    void open.run(async () => {
      setProduct(await get<Product>(`/products/${id}`))
    })
  const add = (
    <Button onClick={() => setPicking(true)}>
      <Icon name="plus" className="h-4 w-4" />
      Оприходовать
    </Button>
  )

  return (
    <div>
      <PageHeader title="Оприходование" subtitle="Товар без накладной и без денег: количество, цена закупки и откуда товар" actions={add} />
      <Card>
        <div className="mb-4 grid grid-cols-2 gap-3 sm:max-w-md">
          <Field label="С">
            <input type="date" value={from} onChange={(e) => setFrom(e.target.value)} />
          </Field>
          <Field label="По">
            <input type="date" value={to} onChange={(e) => setTo(e.target.value)} />
          </Field>
        </div>
        <ErrorBox error={list.error ?? open.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : list.data && list.total === 0 ? (
          <Empty icon="products" action={add}>
            Оприходований за период нет
          </Empty>
        ) : (
          <>
            <Table head={['№', 'Дата', 'Товар', 'Количество', 'Стоимость', 'Комментарий', 'Кто', '']}>
              {list.items.map((d) => (
                <tr key={d.id} data-testid="stock-in-row" className={d.reversed_by ? 'text-slate-400' : ''}>
                  <td className="px-2 py-2 font-medium">
                    {d.number} {d.reversal_of && <Badge tone="amber">сторно</Badge>}
                    {d.reversed_by && <Badge tone="rose">сторнировано</Badge>} <CorrectionBadges mark={marks.data?.get(d.id)} />
                  </td>
                  <td className="whitespace-nowrap px-2 py-2">{formatDateTime(d.created_at)}</td>
                  <td className="px-2 py-2">{d.product_name}</td>
                  <td className="whitespace-nowrap px-2 py-2 tabular-nums">{qtyText(d.unit, d.qty)}</td>
                  <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">{formatSom(d.cost_tyiyn)}</td>
                  <td className="px-2 py-2 text-slate-600">{d.comment}</td>
                  <td className="px-2 py-2">{d.user_name}</td>
                  <td className="px-2 py-2 text-right">
                    <Button variant="ghost" className="px-2 py-1 text-xs" disabled={open.busy} onClick={() => openFor(d.product_id)}>
                      Товар
                    </Button>
                  </td>
                </tr>
              ))}
            </Table>
            <Pager page={list.page} size={list.size} total={list.total} onPage={list.setPage} onSize={list.setSize} label="Страницы оприходований" />
          </>
        )}
      </Card>

      {picking && (
        <Modal title="Оприходовать: какой товар" onClose={() => setPicking(false)}>
          <div className="flex flex-col gap-3" data-stock-in-picker>
            <p className="text-sm text-slate-600">Отсканируйте или найдите товар. Новый товар сначала заведите на экране «Товары».</p>
            <ProductPicker
              placeholder="Сканируйте или найдите товар, который оприходуем"
              onPick={(p) => {
                setPicking(false)
                setProduct(p)
              }}
            />
          </div>
        </Modal>
      )}
      {product && (
        <StockInModal
          product={product}
          onClose={() => setProduct(null)}
          onDone={() => {
            list.reload()
            marks.reload()
          }}
        />
      )}
    </div>
  )
}
