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
  ],
  ...over,
})

let n = 0
const key = () => `k${++n}`

describe('исправление прихода: исходная накладная в форму', () => {
  it('масло канистрами, цена за канистру, наценка от цены продажи', () => {
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
    const all = new Map([['oil', oil], ['f', filter]])
    expect(receiptFixState(doc({ payment: 'paid', paid_now_tyiyn: null }), all, key).payMode).toBe('paid')
    expect(receiptFixState(doc({ paid_now_tyiyn: null, paid_from: null }), all, key)).toMatchObject({ payMode: 'debt', partText: '', paidFrom: '' })
  })

  it('строку нельзя потерять: нет карточки — ошибка', () => {
    expect(() => receiptFixState(doc(), new Map([['oil', oil]]), key)).toThrow(/товар строки/)
  })

  it('масло не целыми канистрами — литрами с ценой литра; на вес — кг (SPEC-36 §13)', () => {
    const scrap = { id: 'g', name: 'Лом', unit: 'g', container_ml: null, sale_price_tyiyn: 12_000 } as Product
    const s = receiptFixState(
      doc({
        lines: [
          { product_id: 'oil', qty: 18_000, cost_tyiyn: 810_000, sale_price_tyiyn: null },
          { product_id: 'g', qty: 129_300, cost_tyiyn: 1_293_000, sale_price_tyiyn: null },
          { product_id: 'oil', qty: 6000, cost_tyiyn: 100_001, sale_price_tyiyn: null },
        ],
      }),
      new Map([
        ['oil', oil],
        ['g', scrap],
      ]),
      key,
    )
    expect(s.lines.map((l) => [l.count, l.mode, l.price, l.markup])).toEqual([
      ['18', 'l', '450', '11,1'],
      ['129,3', undefined, '100', '20,0'],
      ['6', 'l', '166,67', '200,0'],
    ])
    // 6 л по 166,67 с = 1 000,02 с, а было 1 000,01 с — цена литра округлена, проверьте.
    expect(s.rounded).toEqual(['Масло'])
  })
})
