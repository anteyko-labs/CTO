import { describe, expect, it } from 'vitest'
import { qtyInput, saleFixState, type SaleDraftDoc } from './saleFix'
import type { Product, Service } from './types'

const oil = { id: 'oil', name: 'Масло', unit: 'ml', container_ml: 4000, sale_price_tyiyn: 200_000 } as Product
const filter = { id: 'f', name: 'Фильтр', unit: 'piece', container_ml: null, sale_price_tyiyn: 30_000 } as Product
const work = { id: 's', name: 'Замена масла', price_tyiyn: 20_000 } as Service

const doc = (over: Partial<SaleDraftDoc> = {}): SaleDraftDoc => ({
  sale_type: 'service',
  cashier_id: 'c1',
  master_id: 'm1',
  party_id: null,
  contact_id: null,
  vehicle_id: null,
  comment: 'клиент спешил',
  delivery_address: '',
  business_date: '2026-10-18',
  mileage: null,
  mileage_unit: null,
  declined: '',
  lines: [
    { kind: 'pour', gift: false, product_id: 'oil', service_id: null, qty: 3500, unit_price_tyiyn: 50_000 },
    { kind: 'piece', gift: false, product_id: 'f', service_id: null, qty: 2, unit_price_tyiyn: 30_000 },
    { kind: 'service', gift: false, product_id: null, service_id: 's', qty: 1, unit_price_tyiyn: 20_000 },
    { kind: 'piece', gift: true, product_id: 'gone', service_id: null, qty: 1, unit_price_tyiyn: 0 },
  ],
  payments: [{ method: 'cash', amount_tyiyn: 255_000, grams: null, price_per_kg_tyiyn: null, account_id: null }],
  ...over,
})

let n = 0
const key = () => `k${++n}`

describe('исправление чека: исходный чек в кассу', () => {
  it('строки: розлив в литрах, штуки целым, цена в сомах; пропавшая карточка пропускается', () => {
    const s = saleFixState(doc(), new Map([['oil', oil], ['f', filter]]), new Map([['s', work]]), null, key)
    expect(s.lines.map((l) => [l.kind, l.qtyText, l.priceText])).toEqual([
      ['pour', '3,5', '500'],
      ['piece', '2', '300'],
      ['service', '1', '200'],
    ])
    expect(s.saleType).toBe('service')
    expect(s.cashierId).toBe('c1')
    expect(s.masterId).toBe('m1')
    expect(s.businessDate).toBe('2026-10-18')
    expect(s.payMode).toBe('cash')
    expect(s.delivery).toBeNull()
  })

  it('несколько способов — «Смешанная» с суммами; баллы и аккумулятор отдельно; счёт карты', () => {
    const s = saleFixState(
      doc({
        payments: [
          { method: 'cash', amount_tyiyn: 50_000, grams: null, price_per_kg_tyiyn: null, account_id: null },
          { method: 'card', amount_tyiyn: 150_000, grams: null, price_per_kg_tyiyn: null, account_id: 'optima' },
          { method: 'bonus', amount_tyiyn: 5_000, grams: null, price_per_kg_tyiyn: null, account_id: null },
          { method: 'battery', amount_tyiyn: 50_000, grams: 12_500, price_per_kg_tyiyn: 4_000, account_id: null },
        ],
      }),
      new Map(),
      new Map(),
      null,
      key,
    )
    expect(s.payMode).toBe('mixed')
    expect(s.split).toEqual({ cash: '500', card: '1500', transfer: '', invoice: '', debt: '' })
    expect(s.bankPick).toEqual({ card: 'optima', transfer: '', invoice: '' })
    expect(s.bonusAmount).toBe(5_000)
    expect(s.battery).toEqual({ kgText: '12,5', perKgText: '40' })
  })

  it('количество текстом', () => {
    expect(qtyInput('weight', 1250)).toBe('1,25')
    expect(qtyInput('container', 2)).toBe('2')
  })
})
