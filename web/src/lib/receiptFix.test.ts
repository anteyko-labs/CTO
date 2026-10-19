import { describe, expect, it } from 'vitest'
import { receiptFixState, type ReceiptDraftDoc } from './receiptFix'
import type { Product } from './types'

const oil = { id: 'oil', name: 'Масло', unit: 'ml', container_ml: 4000, sale_price_tyiyn: 200_000 } as Product
const filter = { id: 'f', name: 'Фильтр', unit: 'piece', container_ml: null, sale_price_tyiyn: 30_000 } as Product

const doc = (over: Partial<ReceiptDraftDoc> = {}): ReceiptDraftDoc => ({
  supplier_id: 's1',
  payment: 'debt',
  supplier_doc: 'Н-1',
  comment: '',
  paid_from: 'safe',
  paid_now_tyiyn: 30_000,
  lines: [
    { product_id: 'oil', qty: 8000, cost_tyiyn: 300_000, sale_price_tyiyn: 220_000 },
    { product_id: 'f', qty: 3, cost_tyiyn: 1000, sale_price_tyiyn: null },
    { product_id: 'gone', qty: 1, cost_tyiyn: 100, sale_price_tyiyn: null },
  ],
  ...over,
})

let n = 0
const key = () => `k${++n}`

describe('исправление прихода: исходная накладная в форму', () => {
  it('масло канистрами, цена за канистру, наценка от цены продажи; пропавшая карточка пропускается', () => {
    const s = receiptFixState(doc(), new Map([['oil', oil], ['f', filter]]), key)
    expect(s.lines.map((l) => [l.product.id, l.count, l.price, l.salePrice, l.markup])).toEqual([
      ['oil', '2', '1500', '2200', '46,7'],
      ['f', '3', '3,33', '300', '8909,0'],
    ])
    expect(s.rounded).toEqual(['Фильтр'])
    expect(s.payMode).toBe('part')
    expect(s.partText).toBe('300')
    expect(s.paidFrom).toBe('safe')
    expect(s.supplierDoc).toBe('Н-1')
  })

  it('оплачена сразу и вся в долг', () => {
    expect(receiptFixState(doc({ payment: 'paid', paid_now_tyiyn: null }), new Map(), key).payMode).toBe('paid')
    expect(receiptFixState(doc({ paid_now_tyiyn: null, paid_from: null }), new Map(), key)).toMatchObject({ payMode: 'debt', partText: '', paidFrom: '' })
  })
})
