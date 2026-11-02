// Этикетки списком (задача 007): по каждой позиции накладной — сколько этикеток, по умолчанию
// сколько пришло; число правится, 0 — не печатать. Печать — на складе, большой или маленький
// принтер (задачи 006, 014).
import { useState } from 'react'
import type { Product } from '../lib/types'
import { useAction } from '../lib/hooks'
import type { LabelSize } from '../lib/labelJob'
import type { PricePer } from '../lib/labelText'
import { LabelSizeButtons, sendToStation } from './labelTarget'
import { Button, ErrorBox, Modal, Table } from './ui'

export interface LabelListItem {
  product: Product
  copies: number
}

export function LabelListModal({ items, onClose }: { items: LabelListItem[]; onClose: () => void }) {
  const [counts, setCounts] = useState(() => items.map((i) => String(Math.min(Math.max(i.copies, 0), 500))))
  const act = useAction()
  const nums = counts.map((c) => (c === '' ? 0 : Number(c)))
  const bad = nums.some((n) => !Number.isInteger(n) || n < 0 || n > 500)
  const total = nums.reduce((a, n) => a + (Number.isFinite(n) ? n : 0), 0)
  const print = (size: LabelSize, per: PricePer) =>
    void act.run(async () => {
      const list = items.map((i, k) => ({ product: i.product, copies: nums[k] })).filter((i) => i.copies > 0)
      const n = await sendToStation(list, size, per)
      if (n === 0) throw new Error('Нет этикеток для печати: у товаров нет штрихкодов')
      onClose()
    })
  return (
    <Modal title="Этикетки по накладной" wide onClose={onClose}>
      <div className="flex flex-col gap-3" data-testid="label-list">
        <p className="text-sm text-slate-600">Сколько этикеток на каждую позицию — по умолчанию сколько пришло. 0 — не печатать.</p>
        <Table head={['Товар', 'Этикеток']}>
          {items.map((i, k) => (
            <tr key={`${i.product.id}-${k}`}>
              <td className="px-2 py-2">{i.product.name}</td>
              <td className="px-2 py-2">
                <input
                  className="w-24"
                  inputMode="numeric"
                  aria-label={`Этикеток: ${i.product.name}`}
                  data-label-copies={i.product.id}
                  value={counts[k]}
                  onChange={(e) => {
                    const v = e.target.value.replace(/\D/g, '')
                    setCounts((c) => c.map((x, j) => (j === k ? v : x)))
                  }}
                />
              </td>
            </tr>
          ))}
        </Table>
        <ErrorBox error={act.error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <LabelSizeButtons variant="primary" disabled={act.busy || bad || total === 0} onPrint={print} suffix={` (${total})`} />
        </div>
      </div>
    </Modal>
  )
}
