// Единица и объём канистры товара (задача 002, П4; задача 003, Н10; ADR-139). Только владелец.
// Без остатка — сразу; с остатком — списание и приход той же стоимостью одной операцией на сервере.
// Кг и м (ADR-174, ADR-178): со штуками — по «1 шт = X кг/м»; кг ↔ м ↔ л — только без остатка.
import { useState } from 'react'
import { newOpId, post } from '../lib/api'
import { formatLiters, formatSom, litersInput, parseLiters } from '../lib/format'
import { useAction } from '../lib/hooks'
import type { Product } from '../lib/types'
import { byPieceQty, priceAfter, recount, stockText, unitPlan, unitText, zeroOnly, type StockUnit } from '../lib/unitChange'
import { parseQty, perWord, unitShort } from '../lib/units'
import { Button, ErrorBox, Field, Modal, toast } from './ui'

interface UnitOut {
  product: Product
  branches: { branch_id: string; branch_name: string; from_qty: number; to_qty: number }[]
}

export function ProductUnitModal({
  product,
  onClose,
  onSaved,
}: {
  product: Product
  onClose: () => void
  onSaved: (p: Product) => void
}) {
  const fromUnit = product.unit
  const [unit, setUnit] = useState<StockUnit>(fromUnit === 'ml' ? 'ml' : 'piece')
  const [containerL, setContainerL] = useState(product.container_ml ? litersInput(product.container_ml) : '')
  const [pieceL, setPieceL] = useState(product.container_ml ? litersInput(product.container_ml) : '')
  const [keep, setKeep] = useState<'count' | 'volume'>('count')
  /** «1 шт = X кг/м» строкой. */
  const [pieceQtyText, setPieceQtyText] = useState('')
  const [opId] = useState(newOpId)
  const act = useAction()

  // «1 шт = N л» нужно, когда меняются литры и штуки.
  const needsPiece = (fromUnit === 'ml' && unit === 'piece') || (fromUnit === 'piece' && unit === 'ml')
  // «1 шт = X кг/м» — штуки ↔ кг или м; без остатка можно не указывать.
  const pieceQtyUnit = byPieceQty(fromUnit, unit) ? (unit === 'piece' ? fromUnit : unit) : null
  const pieceQty = pieceQtyUnit && pieceQtyText.trim() ? parseQty(pieceQtyUnit, pieceQtyText) : null
  const badPieceQty = pieceQtyUnit !== null && pieceQtyText.trim() !== '' && pieceQty === null
  // Без коэффициента кг и м с другими единицами — только без остатка (ADR-174, ADR-178).
  const mustBeEmpty = zeroOnly(fromUnit, unit, pieceQty !== null) && product.stock_qty !== 0
  const containerMl = unit === 'ml' ? parseLiters(containerL) : null
  const pieceMl = needsPiece ? parseLiters(pieceL) : null
  const plan = unitPlan(fromUnit, product.container_ml, { unit, containerMl, pieceMl, pieceQty, keep })
  const to = typeof plan === 'string' ? null : recount(product.stock_qty, plan)
  const priceTo = typeof plan === 'string' ? null : priceAfter(product.sale_price_tyiyn, plan)
  const problem = badPieceQty
    ? `Сколько ${pieceQtyUnit ? unitShort(pieceQtyUnit) : ''} в одной штуке — число до 3 знаков после запятой`
    : typeof plan === 'string'
      ? plan
      : mustBeEmpty
        ? pieceQtyUnit
          ? `Остаток ${stockText(product.stock_qty, fromUnit)}: укажите, сколько ${unitShort(pieceQtyUnit)} в одной штуке, — или обнулите остаток ревизией`
          : `Остаток ${stockText(product.stock_qty, fromUnit)}: ${unitText(fromUnit, null)} и ${unitText(unit, null)} друг в друга не пересчитываются. Обнулите остаток ревизией, потом меняйте единицу`
        : to === null
        ? `Остаток ${stockText(product.stock_qty, fromUnit)} не переходит в целое число — продайте или спишите ревизией до целого либо выберите другой объём`
        : null

  const save = () =>
    act.run(async () => {
      const out = await post<UnitOut>(`/products/${product.id}/unit`, {
        op_id: opId,
        unit,
        container_ml: containerMl,
        piece_ml: pieceMl,
        ...(pieceQty !== null ? { piece_qty: pieceQty } : {}),
        keep: fromUnit === 'ml' && unit === 'ml' ? keep : undefined,
      })
      toast('Единица товара изменена')
      onSaved(out.product)
    })

  return (
    <Modal title="Единица и объём канистры" onClose={onClose}>
      <div className="flex flex-col gap-3 text-sm" data-testid="unit-modal">
        <div className="text-slate-600">
          Сейчас: {unitText(fromUnit, product.container_ml)}, остаток {stockText(product.stock_qty, fromUnit)}.
        </div>
        <Field label="Учитывать">
          <select data-testid="unit-target" value={unit} onChange={(e) => setUnit(e.target.value as StockUnit)}>
            <option value="piece">в штуках</option>
            <option value="ml">в литрах (канистра и розлив)</option>
            <option value="g">в килограммах</option>
            <option value="mm">в метрах</option>
          </select>
        </Field>
        {pieceQtyUnit && (
          <Field
            label={`1 шт = сколько ${pieceQtyUnit === 'mm' ? 'метров' : 'килограммов'}`}
            hint={product.stock_qty !== 0 ? 'Нужно: остаток пересчитается по нему' : 'Без остатка можно не указывать'}
          >
            <input data-testid="unit-piece-qty" inputMode="decimal" value={pieceQtyText} onChange={(e) => setPieceQtyText(e.target.value)} />
          </Field>
        )}
        {unit === 'ml' && (
          <Field label="Объём канистры, л">
            <input data-testid="unit-container" inputMode="decimal" value={containerL} onChange={(e) => setContainerL(e.target.value)} />
          </Field>
        )}
        {needsPiece && (
          <Field label="1 шт = сколько литров" hint="Обычно — одна канистра">
            <input data-testid="unit-piece" inputMode="decimal" value={pieceL} onChange={(e) => setPieceL(e.target.value)} />
          </Field>
        )}
        {fromUnit === 'ml' && unit === 'ml' && (
          <Field label="Остаток">
            <select data-testid="unit-keep" value={keep} onChange={(e) => setKeep(e.target.value as 'count' | 'volume')}>
              <option value="count">канистр столько же (канистра на самом деле другого объёма)</option>
              <option value="volume">литров столько же (ошиблись только в объёме тары)</option>
            </select>
          </Field>
        )}
        {problem ? (
          <ErrorBox error={problem} />
        ) : (
          to !== null && (
            <div className="rounded-md border border-slate-200 bg-slate-50 px-3 py-2" data-testid="unit-preview">
              <div>
                Остаток: {stockText(product.stock_qty, fromUnit)} → {stockText(to, unit)}
                {to !== product.stock_qty && ' (списание и приход той же стоимостью)'}
              </div>
              {priceTo !== null && priceTo !== product.sale_price_tyiyn && (
                <div>
                  Цена: {formatSom(product.sale_price_tyiyn)} → {formatSom(priceTo)} {unit === 'ml' ? 'за канистру' : unit === 'piece' ? 'за штуку' : perWord(unit)}
                </div>
              )}
              {unit === 'ml' && containerMl && <div>Канистра — {formatLiters(containerMl)}</div>}
            </div>
          )
        )}
        <div className="text-xs text-slate-500">
          Остаток в других филиалах пересчитается так же; без пересчёта (кг, м и литры между собой) остаток должен быть 0 во всех
          филиалах. Прошлые чеки и приходы остаются в прежних единицах; приход до пересчёта
          отменить будет нельзя — только ревизией. Кассы без сети обновятся при связи.
        </div>
        <ErrorBox error={act.error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button data-testid="unit-save" disabled={act.busy || Boolean(problem)} onClick={() => void save()}>
            Пересчитать
          </Button>
        </div>
      </div>
    </Modal>
  )
}
