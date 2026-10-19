import { describe, expect, it } from 'vitest'
import { amountInput, fixedText, oilStockForFix, qtyInput, returnLineFromDraft } from './docFix'

describe('исправление документов: поля формы из исходного', () => {
  it('количество: штуки целым, масло литрами, вес килограммами', () => {
    expect(qtyInput('piece', 3)).toBe('3')
    expect(qtyInput('ml', 8500)).toBe('8,5')
    expect(qtyInput('g', 12_350)).toBe('12,35')
  })

  it('сумма: сомы, пусто без суммы', () => {
    expect(amountInput(123_450)).toBe('1234,50')
    expect(amountInput(null)).toBe('')
    expect(amountInput(undefined)).toBe('')
  })

  it('уведомление с номером нового документа и без него', () => {
    expect(fixedText({ kind_label: 'Выплата', new_number: 15 })).toBe('Исправлено: Выплата → № 15')
    expect(fixedText({ kind_label: 'Начисление', new_number: null })).toBe('Исправлено: Начисление')
  })

  it('строка возврата: заданная сумма не пересчитывается', () => {
    const product = { id: 'p1', unit: 'ml' as const }
    expect(returnLineFromDraft({ product_id: 'p1', qty: 4000, amount_tyiyn: 150_000 }, product, 120_000)).toEqual({
      key: 'fix-p1',
      product,
      qty: '4',
      amount: '1500',
      manual: true,
      price: 120_000,
    })
    expect(returnLineFromDraft({ product_id: 'p1', qty: 500, amount_tyiyn: null }, product, null).manual).toBe(false)
  })

  it('остаток масла для исправления: литры исходного перелива возвращаются источнику', () => {
    const orig = { from_product_id: 'a', to_product_id: 'b', qty_ml: 5000 }
    expect(oilStockForFix('a', 0, orig)).toBe(5000)
    expect(oilStockForFix('b', 7000, orig)).toBe(2000)
    expect(oilStockForFix('c', 1000, orig)).toBe(1000)
    expect(oilStockForFix('a', 1000, null)).toBe(1000)
  })
})
