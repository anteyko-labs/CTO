import { describe, expect, it } from 'vitest'
import { appendCount, countLogText, defaultCountMode, type CountEntry } from './revisionCount'

describe('defaultCountMode', () => {
  it('строку уже считали — прибавить своё, не считали — итог', () => {
    expect(defaultCountMode({ unit: 'piece', container_ml: null, counted_qty: null })).toBe('set')
    expect(defaultCountMode({ unit: 'piece', container_ml: null, counted_qty: 0 })).toBe('set')
    expect(defaultCountMode({ unit: 'piece', container_ml: null, counted_qty: 2 })).toBe('add')
    expect(defaultCountMode({ unit: 'g', container_ml: null, counted_qty: 1500 })).toBe('add')
  })

  it('масло в таре — всегда прибавка (вскрытая канистра литрами)', () => {
    expect(defaultCountMode({ unit: 'ml', container_ml: 4000, counted_qty: null })).toBe('add')
    expect(defaultCountMode({ unit: 'ml', container_ml: 4000, counted_qty: 8000 })).toBe('add')
  })
})

describe('журнал строки', () => {
  const log: CountEntry[] = [
    { by: 'Айбек', mode: 'add', qty: 2 },
    { by: 'Нурлан', mode: 'add', qty: 1 },
    { by: 'Азиз', mode: 'set', qty: 5 },
  ]

  it('«Айбек +2 шт, Нурлан +1 шт, Азиз = 5 шт»', () => {
    expect(countLogText(log, (q) => `${q} шт`)).toBe('Айбек +2 шт, Нурлан +1 шт, Азиз = 5 шт')
    expect(countLogText([{ by: 'Азиз', mode: 'clear', qty: 0 }], String)).toBe('Азиз убрал')
  })

  it('прибавки одного человека подряд складываются, итог — отдельной записью', () => {
    let l: CountEntry[] = []
    l = appendCount(l, { by: 'Айбек', mode: 'add', qty: 1 })
    l = appendCount(l, { by: 'Айбек', mode: 'add', qty: 1 })
    l = appendCount(l, { by: 'Нурлан', mode: 'add', qty: 1 })
    l = appendCount(l, { by: 'Азиз', mode: 'set', qty: 5 })
    l = appendCount(l, { by: 'Азиз', mode: 'set', qty: 6 })
    expect(l).toEqual([...log, { by: 'Азиз', mode: 'set', qty: 6 }])
  })
})
