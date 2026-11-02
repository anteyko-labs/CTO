import { describe, expect, it } from 'vitest'
import { addIsEmpty, appendCount, countLogText, countSummary, defaultCountMode, mineOf, type CountEntry } from './revisionCount'

describe('defaultCountMode', () => {
  it('всегда «+ добавить»: каждый вписывает найденное в своём месте, «= итог» — только явным выбором (задача 005, Н7)', () => {
    expect(defaultCountMode()).toBe('add')
  })
})

describe('addIsEmpty', () => {
  it('0 в непосчитанной строке — «не нашли», записывается; в посчитанной +0 и пустое — не уходят', () => {
    expect(addIsEmpty(0, null)).toBe(false)
    expect(addIsEmpty(0, 3)).toBe(true)
    expect(addIsEmpty(0, 0)).toBe(true)
    expect(addIsEmpty(null, null)).toBe(true)
    expect(addIsEmpty(2, null)).toBe(false)
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

describe('кто сколько добавил (задача 005, Н7)', () => {
  const sht = (q: number) => `${q} шт`

  it('«Азиз +6 шт, Алексей +4 шт = 10 шт»: прибавки одного входа складываются и не подряд', () => {
    const log: CountEntry[] = [
      { by: 'Азиз', user_id: 'a', mode: 'add', qty: 2 },
      { by: 'Алексей', user_id: 'b', mode: 'add', qty: 4 },
      { by: 'Азиз', user_id: 'a', mode: 'add', qty: 4 },
    ]
    expect(countSummary(log, 10, sht)).toBe('Азиз +6 шт, Алексей +4 шт = 10 шт')
  })

  it('с последнего «= итог»: прежние прибавки итог уже не составляют', () => {
    const log: CountEntry[] = [
      { by: 'Азиз', user_id: 'a', mode: 'add', qty: 6 },
      { by: 'Алексей', user_id: 'b', mode: 'set', qty: 5 },
      { by: 'Азиз', user_id: 'a', mode: 'add', qty: 4 },
    ]
    expect(countSummary(log, 9, sht)).toBe('Алексей = 5 шт, Азиз +4 шт = 9 шт')
    expect(countSummary(log.slice(0, 2), 5, sht)).toBe('Алексей = 5 шт')
  })

  it('после «убрал» — только новые прибавки', () => {
    const log: CountEntry[] = [
      { by: 'Азиз', user_id: 'a', mode: 'add', qty: 6 },
      { by: 'Азиз', user_id: 'a', mode: 'clear', qty: 0 },
      { by: 'Алексей', user_id: 'b', mode: 'add', qty: 3 },
    ]
    expect(countSummary(log, 3, sht)).toBe('Алексей +3 шт = 3 шт')
  })

  it('одинаковые имена разных входов не сливаются; прежний ответ без входа — по имени', () => {
    const log: CountEntry[] = [
      { by: 'Азиз', user_id: 'a', mode: 'add', qty: 1 },
      { by: 'Азиз', user_id: 'z', mode: 'add', qty: 2 },
    ]
    expect(countSummary(log, 3, sht)).toBe('Азиз +1 шт, Азиз +2 шт = 3 шт')
    expect(appendCount(log, { by: 'Азиз', user_id: 'z', mode: 'add', qty: 1 })).toEqual([log[0], { ...log[1], qty: 3 }])
    expect(appendCount([{ by: 'Азиз', mode: 'add', qty: 1 }], { by: 'Азиз', mode: 'add', qty: 1 })).toEqual([{ by: 'Азиз', mode: 'add', qty: 2 }])
  })
})

describe('mineOf — своё число строки (задача 007)', () => {
  const e = (user_id: string, mode: 'add' | 'set' | 'clear', qty: number) => ({ by: user_id, user_id, mode, qty })
  it('сумма своих прибавок, чужие не считаются', () => {
    expect(mineOf([e('a', 'add', 5), e('b', 'add', 4), e('a', 'add', -2)], 'a')).toBe(3)
    expect(mineOf([e('a', 'add', 5), e('b', 'add', 4)], 'b')).toBe(4)
  })
  it('не считал сам — null; «не нашли» — 0', () => {
    expect(mineOf([e('b', 'add', 4)], 'a')).toBeNull()
    expect(mineOf([e('a', 'add', 0)], 'a')).toBe(0)
  })
  it('после «= итог» или «убрал» — с нуля', () => {
    expect(mineOf([e('a', 'add', 5), e('o', 'set', 8), e('a', 'add', 1)], 'a')).toBe(1)
    expect(mineOf([e('a', 'add', 5), e('o', 'clear', 0)], 'a')).toBeNull()
  })
})
