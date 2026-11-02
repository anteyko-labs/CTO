import { describe, expect, it } from 'vitest'
import {
  addCounts,
  candidateText,
  docText,
  duplicateOf,
  newLines,
  recognitionText,
  splitLines,
  suggestFor,
  totalMismatch,
  type DraftLine,
} from './receiptDrafts'

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
  remembered: false,
  candidates: [],
  photo: 1,
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
    // 10 л по 4 л — строка в литрах: количество в мл, цена — за литр из накладной (SPEC-36 §13).
    expect(odd).toMatchObject({ count: 10_000, each: 45_000, measure: 'l' })
    expect(nb(odd.note)).toContain('не делится')
    const bySum = suggestFor(line({ quantity: '18', unit: 'л', qty_milli: 18_000, liters: true, sum_tyiyn: 810_000 }), canister)
    expect(bySum).toMatchObject({ count: 18_000, each: 45_000, measure: 'l' })
  })

  it('на вес — килограммы и цена за кг', () => {
    const scrap = { unit: 'g', container_ml: null } as const
    expect(suggestFor(line({ quantity: '12,5', unit: 'кг', qty_milli: 12_500, price_tyiyn: 10_000 }), scrap)).toEqual({
      count: 12_500,
      each: 10_000,
      note: null,
      measure: 'kg',
    })
  })

  it('в длину — метры и цена за м; другая единица накладной — заметка (ADR-175)', () => {
    const hose = { unit: 'mm', container_ml: null } as const
    // 25 м на 3 000 с: 120 с за м — одним округлением из суммы.
    expect(suggestFor(line({ quantity: '25', unit: 'м', qty_milli: 25_000, sum_tyiyn: 300_000 }), hose)).toEqual({
      count: 25_000,
      each: 12_000,
      note: null,
      measure: 'm',
    })
    const cm = suggestFor(line({ quantity: '150', unit: 'см', qty_milli: 150_000, price_tyiyn: 100 }), hose)
    expect(cm.measure).toBe('m')
    expect(nb(cm.note)).toContain('проверьте единицу')
    const grams = suggestFor(line({ quantity: '500', unit: 'г', qty_milli: 500_000, price_tyiyn: 100 }), { unit: 'g', container_ml: null })
    expect(nb(grams.note)).toContain('строка в кг')
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

describe('задача 002: ещё фото и варианты товара', () => {
  it('несколько точных совпадений — в строки без товара, на выбор', () => {
    const { found, missing } = splitLines({ lines: [line({ match: 'several', candidates: [] })] })
    expect(found.length).toBe(0)
    expect(missing.length).toBe(1)
  })

  it('«Ещё фото» добавляет только строки нового фото', () => {
    const lines = [line({ no: 1, photo: 1 }), line({ no: 2, photo: 2 }), line({ no: 3, photo: 2 })]
    expect(newLines({ lines, added_photo: null }).map((l) => l.no)).toEqual([1, 2, 3])
    expect(newLines({ lines, added_photo: 2 }).map((l) => l.no)).toEqual([2, 3])
  })

  it('тот же товар уже есть в приходе', () => {
    const rows = [
      { key: 'a', product: { id: 'p1' } },
      { key: 'b', product: { id: 'p2' } },
    ]
    expect(duplicateOf(rows, 'p2')?.key).toBe('b')
    expect(duplicateOf(rows, 'p2', 'b')).toBeNull()
    expect(duplicateOf(rows, 'p3')).toBeNull()
  })

  it('сложить количество: целые и тысячные, без дробных чисел', () => {
    expect(addCounts('2', '3', 'whole')).toBe('5')
    expect(addCounts('2', '1,5', 'whole')).toBeNull()
    expect(addCounts('', '1', 'whole')).toBeNull()
    expect(addCounts('2,5', '0,75', 'milli')).toBe('3,25')
    expect(addCounts('0,1', '0,2', 'milli')).toBe('0,3')
    expect(addCounts('1', 'abc', 'milli')).toBeNull()
  })

  it('вариант товара: артикул, остаток, прошлый приход', () => {
    expect(nb(candidateText({ article: 'BKR6E', last_purchase_price_tyiyn: 25_000 }, '4 шт'))).toBe('BKR6E · остаток 4 шт · прошлый приход 250 с')
    expect(candidateText({ article: '', last_purchase_price_tyiyn: null }, '0 шт')).toBe('остаток 0 шт')
  })
})
