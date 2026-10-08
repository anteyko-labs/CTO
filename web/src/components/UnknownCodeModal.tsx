// Окно незнакомого штрихкода: привязать код к заведённому товару или завести новый (ADR-026).
import { useState } from 'react'
import { get, newOpId, post, qs } from '../lib/api'
import { useAction, useDebounced, useLoad } from '../lib/hooks'
import type { Product } from '../lib/types'
import { Button, Card, Empty, ErrorBox, Field, Loading, Modal } from './ui'
import { stockText } from './ProductPicker'

/**
 * Незнакомый штрихкод: его либо привязывают к уже заведённому товару, либо заводят новый.
 * Привязка — главный способ не разводить карточки-дубли (ADR-026).
 */
export function UnknownCodeModal({
  code,
  onClose,
  onLinked,
  onCreateNew,
}: {
  code: string
  onClose: () => void
  onLinked: (p: Product) => void
  onCreateNew: () => void
}) {
  const [q, setQ] = useState('')
  const query = useDebounced(q.trim(), 200)
  const found = useLoad(
    () => (query.length < 2 ? Promise.resolve<Product[]>([]) : get<Product[]>(`/products${qs({ q: query, limit: 10 })}`)),
    [query],
  )
  const link = useAction()

  const attach = (p: Product) =>
    link.run(async () => {
      await post(`/products/${p.id}/barcodes`, { op_id: newOpId(), code })
      onLinked(await get<Product>(`/products/${p.id}`))
    })

  return (
    <Modal title={`Код ${code} не найден`} onClose={onClose}>
      <div className="flex flex-col gap-3">
        <p className="text-sm text-slate-600">
          Если товар уже заведён, привяжите код к нему — так в справочнике не появится второй такой же товар.
        </p>
        <Field label="Найти товар">
          <input autoFocus placeholder="Название, бренд, артикул" value={q} onChange={(e) => setQ(e.target.value)} />
        </Field>
        <ErrorBox error={found.error ?? link.error} />
        {found.loading && !found.data ? (
          <Loading />
        ) : query.length < 2 ? (
          <Empty>Введите хотя бы две буквы</Empty>
        ) : (found.data ?? []).length === 0 ? (
          <Empty>Ничего не нашлось</Empty>
        ) : (
          <ul className="flex flex-col gap-2">
            {(found.data ?? []).map((p) => (
              <li key={p.id}>
                <Card className="flex items-center justify-between gap-3 py-2">
                  <div>
                    <div className="font-medium">{p.name}</div>
                    <div className="text-xs text-slate-500">
                      {p.brand || '—'} · {p.article || 'без артикула'} · {stockText(p)}
                    </div>
                  </div>
                  <Button variant="secondary" disabled={link.busy} onClick={() => void attach(p)}>
                    Привязать код
                  </Button>
                </Card>
              </li>
            ))}
          </ul>
        )}
        <div className="flex justify-end gap-2 border-t border-slate-200 pt-3">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button onClick={onCreateNew}>Новый товар</Button>
        </div>
      </div>
    </Modal>
  )
}
