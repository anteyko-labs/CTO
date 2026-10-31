// «Указать цену закупки» владельцем (ADR-062, ADR-128): товар продан без цены или остаток без
// стоимости. Из «Товаров» и из блока «Себестоимость» карточки (задача 003, Н7). Масло — «за кан. | за л».
import { useState } from 'react'
import { newOpId, post } from '../lib/api'
import { formatSom } from '../lib/format'
import { useAction } from '../lib/hooks'
import { costPer, entryUnitPrice, type PriceEntry, type PricePer } from '../lib/purchasePrice'
import type { Product } from '../lib/types'
import { PurchasePriceInput } from './PurchasePriceInput'
import { Button, ErrorBox, Modal, toast } from './ui'

export function PurchasePriceModal({
  product,
  onClose,
  onDone,
}: {
  product: Product
  onClose: () => void
  onDone: () => void
}) {
  const [opId] = useState(newOpId)
  const [entry, setEntry] = useState<PriceEntry>({ text: '', per: 'can' })
  const [shown, setShown] = useState<PricePer>('can')
  const act = useAction()
  const tare = product.unit === 'ml' && product.container_ml ? product.container_ml : null
  const save = () => {
    const price = entryUnitPrice(entry, tare)
    if (typeof price !== 'number' || price <= 0) {
      act.setError('Укажите цену закупки')
      return
    }
    void act.run(async () => {
      const r = await post<{ lines: number; accrued_tyiyn: number }>(`/stock/${product.id}/purchase-price`, { op_id: opId, price_tyiyn: price })
      toast(`Цена закупки указана: строк чеков ${r.lines}, кассирам начислено ${formatSom(r.accrued_tyiyn)}`)
      onDone()
    })
  }
  return (
    <Modal title={`Цена закупки · ${product.name}`} onClose={onClose}>
      <div className="flex flex-col gap-3">
        <p className="text-sm text-slate-600">
          {product.cost_pending
            ? 'Товар продали без цены закупки: себестоимость этих строк неизвестна, и 2 % кассиру с них ещё не начислены. Укажите, почём товар закупается, — процент начислится сегодня, прошлые дни не изменятся.'
            : 'Почём товар закупается: по этой цене пойдёт продажа без остатка, а остаток без цены (излишек ревизии) оценится по ней. Цену прихода она не меняет.'}
        </p>
        <PurchasePriceInput
          id="purchase-price"
          label="Цена закупки"
          autoFocus
          required
          entry={entry}
          shown={shown}
          containerMl={tare}
          unitWord={costPer(product).replace(/^за шт$/, 'за штуку')}
          sale={product.sale_price_tyiyn}
          onChange={setEntry}
          onShown={setShown}
          onEnter={save}
        />
        <ErrorBox error={act.error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={act.busy || !entry.text.trim()} onClick={save}>
            Сохранить
          </Button>
        </div>
      </div>
    </Modal>
  )
}
