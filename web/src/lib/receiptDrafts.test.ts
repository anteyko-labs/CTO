import { describe, expect, it } from 'vitest'
import { docText, recognitionText, splitLines, suggestFor, totalMismatch, type DraftLine } from './receiptDrafts'

const nb = (s: string | null) => (s ?? '').replace(/ /g, ' ')

const line = (p: Partial<DraftLine>): DraftLine => ({
  no: 1,
  name: 'Масло',
  code: '',
  barcode: '',
  quantity: '1',
  unit: 'шт',
  price: '',
  sum: '',
  qty_milli: 1000,
  liters: false,
  price_tyiyn: null,
  sum_tyiyn: null,
  match: 'none',
  product: null,
  count: null,
  each_tyiyn: null,
  note: null,
  ...p,
})

const canister = { unit: 'ml' as const, container_ml: 4000 }
const piece = { unit: 'piece' as const, container_ml: null }

describe('накладная по фото', () => {
  it('литры масла — в канистры, цена литра × объём', () => {
    const s = suggestFor(line({ quantity: '20', unit: 'л', qty_milli: 20_000, liters: true, price_tyiyn: 45_000 }), canister)
    expect(s.count).toBe(5)
    expect(s.each).toBe(180_000)
    expect(nb(s.note)).toBe('20 л = 5 кан. по 4 л')
    const odd = suggestFor(line({ quantity: '10', unit: 'л', qty_milli: 10_000, liters: true, price_tyiyn: 45_000 }), canister)
    expect(odd.count).toBeNull()
    expect(nb(odd.note)).toContain('не делится')
  })

  it('штуки, дробное количество, цена по сумме', () => {
    expect(suggestFor(line({ qty_milli: 3000, price_tyiyn: 35_000 }), piece)).toEqual({ count: 3, each: 35_000, note: null })
    // Канистры штуками — как есть.
    expect(suggestFor(line({ qty_milli: 2000, price_tyiyn: 180_000 }), canister).count).toBe(2)
    const half = suggestFor(line({ quantity: '1.5', qty_milli: 1500, price_tyiyn: 100 }), piece)
    expect(half.count).toBeNull()
    expect(half.note).toContain('укажите целое')
    const bySum = suggestFor(line({ qty_milli: 4000, sum_tyiyn: 100_000 }), piece)
    expect(bySum.each).toBe(25_000)
    expect(bySum.note).toContain('по сумме')
  })

  it('строки без товара — отдельно', () => {
    const p = { id: 'p' } as DraftLine['product']
    const { found, missing } = splitLines({
      lines: [line({ match: 'exact', product: p }), line({ match: 'similar', product: p }), line({ match: 'none' })],
    })
    expect(found.length).toBe(2)
    expect(missing.length).toBe(1)
  })

  it('итог накладной против суммы строк', () => {
    expect(totalMismatch(1_120_000, 1_120_000)).toBeNull()
    expect(totalMismatch(null, 100)).toBeNull()
    expect(nb(totalMismatch(1_120_000, 1_070_000))).toBe('Итог накладной 11 200 с, сумма строк 10 700 с — расходится на 500 с')
  })

  it('номер и дата документа, состояние распознавания', () => {
    const inv = { readable: true, supplier_name: '', supplier_inn: '', doc_number: '15', doc_date: '2026-10-20', total: '' }
    expect(docText(inv)).toBe('15 от 20.10.2026')
    expect(docText({ ...inv, doc_date: '' })).toBe('15')
    expect(docText(null)).toBe('')
    expect(recognitionText({ recognition: 'done', lines: 3 })).toBeNull()
    expect(recognitionText({ recognition: 'done', lines: 0 })).toBe('не удалось распознать')
    expect(recognitionText({ recognition: 'off', lines: 0 })).toContain('не настроено')
  })
})
