// Правило подарка (SPEC-11): статус в списке и заполнение формы для правки. Выключенное правило,
// у которого порог или количество подарка не перешли в новую единицу товара целым числом, сервер
// округляет и помечает `needs_fix` (задача 003, хвост ADR-139): включается только после правки.
import type { GiftRule } from './types'
import { qtyInputText } from './units'

/** Статус правила словами — для экрана и выгрузки. */
export function giftRuleStatus(r: Pick<GiftRule, 'active' | 'needs_fix'>): string {
  if (r.needs_fix) return 'выключено — поправьте порог'
  return r.active ? 'включено' : 'выключено'
}

/** Порог правила в поле формы: литры у масла («3,5»), кг и м — так же, штуки — целым; пусто — без порога. */
export function giftMinText(r: Pick<GiftRule, 'min_units' | 'trigger_unit'>): string {
  if (r.min_units <= 0) return ''
  return qtyInputText(r.trigger_unit, r.min_units)
}

/** Количество каждого подарка правила: при пересохранении не сбрасывается в 1. */
export function giftQtys(r: Pick<GiftRule, 'items'>): Record<string, number> {
  return Object.fromEntries(r.items.map((i) => [i.gift_product_id, i.gift_qty]))
}
