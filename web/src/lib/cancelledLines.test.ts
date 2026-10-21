import { describe, expect, it } from 'vitest'
import { cancelledBody, forServer, removedLine, unremove, type CartLineView } from './cancelledLines'
import { isSaleItem, type OutboxItem } from './offline'

const at = new Date('2026-10-21T05:00:00Z')
const view = (over: Partial<CartLineView> = {}): CartLineView => ({
  key: 'k1',
  kind: 'piece',
  product_id: 'p1',
  service_id: null,
  name: 'Фильтр',
  qty: 2,
  price: 50_000,
  ...over,
})

describe('отменённые товары (SPEC-30 §8, ADR-099)', () => {
  it('удалённая строка: количество и цена как в чеке, время удаления', () => {
    expect(removedLine(view(), 'remove', at)).toEqual({
      key: 'k1',
      reason: 'remove',
      kind: 'piece',
      product_id: 'p1',
      service_id: null,
      name: 'Фильтр',
      qty: 2,
      unit_price_tyiyn: 50_000,
      removed_at: '2026-10-21T05:00:00.000Z',
    })
  })

  it('строку без количества не пишем, подарок — по нулевой цене', () => {
    expect(removedLine(view({ qty: null }), 'remove', at)).toBeNull()
    expect(removedLine(view({ qty: 0 }), 'clear', at)).toBeNull()
    expect(removedLine(view({ gift: true }), 'clear', at)?.unit_price_tyiyn).toBe(0)
    expect(removedLine(view({ price: null }), 'clear', at)?.unit_price_tyiyn).toBe(0)
  })

  it('«Вернуть» снимает запись только этой строки', () => {
    const a = removedLine(view(), 'remove', at)
    const b = removedLine(view({ key: 'k2' }), 'remove', at)
    if (!a || !b) throw new Error('строки')
    expect(unremove([a, b], 'k1')).toEqual([b])
    expect(unremove([a, b], 'нет')).toEqual([a, b])
  })

  it('на сервер — без ключа строки кассы; отдельная запись со сменой и op_id', () => {
    const a = removedLine(view(), 'clear', at)
    if (!a) throw new Error('строка')
    expect(forServer([a])[0]).not.toHaveProperty('key')
    const body = cancelledBody([a], 'shift-1', 'op-1', at)
    expect(body).toEqual({ op_id: 'op-1', shift_id: 'shift-1', client_time: '2026-10-21T05:00:00.000Z', lines: forServer([a]) })
  })

  it('запись очереди с адресом — не чек: экраны очереди её не считают', () => {
    const base: OutboxItem = { op_id: 'a', temp_no: '', created_at: '', status: 'pending', attempts: 0, last_error: '', body: {} }
    expect(isSaleItem(base)).toBe(true)
    expect(isSaleItem({ ...base, path: '/sales/cancelled-lines' })).toBe(false)
  })
})
