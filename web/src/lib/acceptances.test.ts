import { describe, expect, it } from 'vitest'
import {
  acceptBody,
  acceptProblems,
  acceptedInput,
  diffCount,
  diffText,
  emptyMarks,
  inputKind,
  lineByCode,
  lineState,
  overLines,
  parseAccepted,
  qtyWords,
  scanStep,
  type AcceptanceLine,
  type Marks,
} from './acceptances'

const nb = (s: string) => s.replace(/ /g, ' ')

const line = (p: Partial<AcceptanceLine>): AcceptanceLine => ({
  line_no: 1,
  product_id: 'p1',
  name: 'Фильтр W712/75',
  article: 'W712/75',
  barcodes: ['4011558'],
  unit: 'piece',
  container_ml: null,
  invoice_qty: 20,
  line_cost_tyiyn: null,
  sale_price_tyiyn: null,
  accepted_qty: null,
  accepted_product_id: null,
  accepted_name: null,
  accepted_container_ml: null,
  reason: null,
  comment: '',
  ...p,
})

const oil = line({ line_no: 2, product_id: 'p2', name: 'Shell 4 л', unit: 'ml', container_ml: 4000, invoice_qty: 20_000, barcodes: ['870'] })
const loose = line({ line_no: 3, product_id: 'p3', name: 'Масло в розлив', unit: 'ml', container_ml: 4000, invoice_qty: 3500, barcodes: [] })
const kg = line({ line_no: 4, product_id: 'p4', name: 'Аккумулятор лом', unit: 'g', invoice_qty: 12_500, barcodes: [] })

describe('показ и ввод количества', () => {
  it('штуки, канистры, литры, кг', () => {
    expect(qtyWords(line({}), 20)).toBe('20 шт')
    expect(nb(qtyWords(oil, 20_000))).toBe('5 кан. по 4 л')
    expect(nb(qtyWords(loose, 3500))).toBe('3,5 л')
    expect(nb(qtyWords(kg, 12_500))).toBe('12,5 кг')
  })
  it('канистры и штуки — целые, литры и кг — до 3 знаков', () => {
    expect(inputKind(line({}))).toBe('int')
    expect(inputKind(oil)).toBe('int')
    expect(inputKind(loose)).toBe('decimal')
    expect(parseAccepted(line({}), '18')).toBe(18)
    expect(parseAccepted(line({}), '1,5')).toBeNull()
    expect(parseAccepted(oil, '4')).toBe(16_000)
    expect(parseAccepted(loose, '2,25')).toBe(2250)
    expect(parseAccepted(kg, '12,345')).toBe(12_345)
    expect(parseAccepted(kg, '')).toBeNull()
    expect(acceptedInput(oil, 16_000)).toBe('4')
    expect(acceptedInput(loose, 2250)).toBe('2,25')
    expect(acceptedInput(line({}), null)).toBe('')
  })
  it('сканер: штука или канистра', () => {
    expect(scanStep(line({}))).toBe(1)
    expect(scanStep(oil)).toBe(4000)
    expect(scanStep(kg)).toBeNull()
    expect(lineByCode([line({}), oil], '870')?.line_no).toBe(2)
    expect(lineByCode([line({}), oil], 'W712/75')?.line_no).toBe(1)
    expect(lineByCode([line({}), oil], 'нет')).toBeNull()
  })
})

describe('отметки', () => {
  const lines = [line({}), oil]
  const marks = (m: Marks['lines']): Marks => ({ ...emptyMarks(), lines: m })

  it('цвет и подпись строки', () => {
    const l = line({})
    expect(lineState(l, undefined)).toBe('none')
    expect(lineState(l, { qty: 20 })).toBe('ok')
    expect(lineState(l, { qty: 18 })).toBe('less')
    expect(diffText(l, { qty: 18 })).toBe('18 из 20')
    expect(lineState(l, { qty: 22 })).toBe('more')
    expect(diffText(l, { qty: 22 })).toBe('+2 сверх накладной')
    expect(diffText(l, { qty: 0, reason: 'missing' })).toBe('0 — не привезли')
    expect(diffText(oil, { qty: 8000 })).toBe('2 из 5')
    expect(lineState(l, { qty: 20, product: { id: 'x', name: 'Аналог', container_ml: null } })).toBe('less')
  })

  it('«Принять» — все строки отмечены, у недостачи причина, хоть что-то принято', () => {
    expect(acceptProblems(lines, marks({ 1: { qty: 20 } }))).toEqual(['отметьте все строки: не отмечено 1'])
    expect(acceptProblems(lines, marks({ 1: { qty: 18 }, 2: { qty: 20_000 } }))[0]).toContain('выберите причину')
    expect(acceptProblems(lines, marks({ 1: { qty: 18, reason: 'damaged' }, 2: { qty: 20_000 } }))).toEqual([])
    expect(acceptProblems(lines, marks({ 1: { qty: 0, reason: 'missing' }, 2: { qty: 0, reason: 'missing' } }))).toEqual([
      'ничего не принято — если товар не привезли, «Отказ от приёмки»',
    ])
  })

  it('больше накладной — подтверждение; расхождения считаются', () => {
    const m = marks({ 1: { qty: 22 }, 2: { qty: 20_000 } })
    expect(overLines(lines, m)).toEqual(['Фильтр W712/75 — 22 из 20'])
    expect(diffCount(lines, m)).toBe(1)
  })

  it('тело запроса: принято, замена, причина только у недостачи', () => {
    const m: Marks = {
      lines: {
        1: { qty: 18, reason: 'missing', comment: ' двух нет ' },
        2: { qty: 20_000, reason: 'other', product: { id: 'p9', name: 'Mobil 4 л', container_ml: 4000 } },
      },
      extras: [{ name: ' Щётки ', barcode: '', qty: 2 }],
      comment: '',
    }
    expect(acceptBody(lines, m)).toEqual({
      lines: [
        { line_no: 1, accepted_qty: 18, reason: 'missing', comment: 'двух нет' },
        { line_no: 2, accepted_qty: 20_000, product_id: 'p9', comment: '' },
      ],
      extras: [{ name: 'Щётки', barcode: '', qty: 2 }],
      comment: '',
    })
  })
})
