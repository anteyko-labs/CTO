import { Button, Empty, Modal } from './ui'
import type { GiftRule } from '../lib/types'

/** Пробили товар с подарками — касса сразу спрашивает, что дарим (SPEC-11). */
export function GiftPicker({
  rule,
  onPick,
  onClose,
}: {
  rule: GiftRule
  onPick: (gift: { product_id: string; qty: number; name: string }) => void
  onClose: () => void
}) {
  return (
    <Modal title="Выберите подарок" onClose={onClose}>
      <div className="flex flex-col gap-2">
        <div className="text-sm text-slate-600">К покупке «{rule.trigger_name}» можно добавить подарок:</div>
        {rule.items.length === 0 ? (
          <Empty>Список подарков пуст</Empty>
        ) : (
          rule.items.map((i) => (
            <Button
              key={i.gift_product_id}
              variant="secondary"
              className="justify-between"
              onClick={() => onPick({ product_id: i.gift_product_id, qty: i.gift_qty, name: i.name })}
            >
              <span>{i.name}</span>
              <span className="text-xs text-slate-500">{i.gift_qty} шт</span>
            </Button>
          ))
        )}
        <div className="flex justify-end pt-2">
          <Button variant="ghost" onClick={onClose}>
            Без подарка
          </Button>
        </div>
      </div>
    </Modal>
  )
}
