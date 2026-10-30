// Запомненные строки накладных поставщика (задача 002, П3): «название строки у поставщика → товар».
// Выбор товара в «Новом приходе» по фото запоминается; в следующий раз такая строка совпадает точно.
// Удалить — право «Товары» или владелец; изменения — в журнале действий.
import { get, newOpId, post } from '../lib/api'
import { formatDateTime } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { useCan } from '../lib/permissions'
import { Button, Empty, ErrorBox, Loading, Table } from './ui'

export interface SupplierLine {
  id: string
  supplier_id: string
  name: string
  product_id: string
  product_name: string
  product_article: string
  user_name: string
  updated_at: string
}

export function SupplierLineMap({ supplierId }: { supplierId: string }) {
  const can = useCan()
  const mayRemove = can('products')
  const rows = useLoad(() => get<SupplierLine[]>(`/suppliers/${supplierId}/line-map`), [supplierId])
  const act = useAction()
  const remove = (r: SupplierLine) => {
    if (!window.confirm(`Забыть «${r.name}» → ${r.product_name}? В следующий раз товар для этой строки выберут заново.`)) return
    void act.run(async () => {
      await post(`/supplier-line-map/${r.id}/remove`, { op_id: newOpId() })
      rows.reload()
    })
  }
  const list = rows.data ?? []
  return (
    <div data-testid="supplier-line-map">
      <h2 className="mb-1 font-semibold">Запомненные строки накладных</h2>
      <p className="mb-3 text-xs text-slate-500">
        Выбор товара для строки накладной по фото запоминается: в следующий раз такая строка этого поставщика находит товар сразу.
      </p>
      <ErrorBox error={rows.error ?? act.error} />
      {rows.loading && !rows.data ? (
        <Loading />
      ) : list.length === 0 ? (
        <Empty>Пока ничего не запомнено — выберите товар для строки в «Новом приходе» по фото</Empty>
      ) : (
        <Table head={['В накладной', 'Товар', 'Кто и когда', '']}>
          {list.map((r) => (
            <tr key={r.id} data-testid="supplier-line">
              <td className="px-2 py-2">«{r.name}»</td>
              <td className="px-2 py-2">
                {r.product_name}
                {r.product_article && <span className="ml-2 text-xs text-slate-500">{r.product_article}</span>}
              </td>
              <td className="px-2 py-2 text-xs text-slate-500">
                {r.user_name}, {formatDateTime(r.updated_at)}
              </td>
              <td className="px-2 py-2 text-right">
                {mayRemove && (
                  <Button variant="ghost" className="px-2 py-1 text-xs" disabled={act.busy} onClick={() => remove(r)}>
                    Забыть
                  </Button>
                )}
              </td>
            </tr>
          ))}
        </Table>
      )}
    </div>
  )
}
