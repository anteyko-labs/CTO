// Совместный пересчёт строки ревизии с нескольких устройств (задача 003, Н14): итог для сверки
// «= итог», op_id ввода, свежесть показа, единицы найденного товара.
import { describe, expect, it } from 'vitest'
import {
  expectedBody,
  expectedQuery,
  freshRevision,
  InputOps,
  inputKey,
  LostOps,
  mergeLines,
  createCounted,
  OwnWrites,
  patchIfNewer,
  qtyInUnits,
} from './revisionCount'

describe('итог для сверки «= итог» и очистки', () => {
  it('видел 3, свой скан ещё шёл, другой прибавил 5 — сверка с 4, а не с 9 из ответа', () => {
    const own = new OwnWrites()
    // Экран показывает 3; свой скан +1 ушёл.
    const scan = own.start('p', 'add', 1)
    // Пока скан шёл, человек начал вписывать итог: видел 3.
    own.see('p', 3)
    // Ответ скана — 9 (другой прибавил 5): своя прибавка записана.
    own.settle('p', scan, true)
    const set = own.start('p', 'set', 4)
    expect(own.base('p', set)).toBe(4)
  })

  it('свои записи до начала ввода уже на экране — не прибавляются второй раз', () => {
    const own = new OwnWrites()
    const a = own.start('p', 'add', 1)
    own.settle('p', a, true)
    own.see('p', 4)
    expect(own.base('p', own.start('p', 'set', 10))).toBe(4)
  })

  it('свой «= итог» и очистка, пока поле правили дальше, — сверка с ними', () => {
    const own = new OwnWrites()
    own.see('p', null)
    const s = own.start('p', 'set', 12_500)
    const s2 = own.start('p', 'set', 12_600)
    own.settle('p', s, true)
    expect(own.base('p', s2)).toBe(12_500)
    own.settle('p', s2, true)
    const c = own.start('p', 'clear', 0)
    expect(own.base('p', c)).toBe(12_600)
    own.settle('p', c, true)
    expect(own.base('p', own.start('p', 'add', 1))).toBeNull()
  })

  it('не записанная своя прибавка в итог не входит; ввод не начинали — сверять не с чем', () => {
    const own = new OwnWrites()
    own.see('p', 2)
    const a = own.start('p', 'add', 1)
    own.settle('p', a, false)
    expect(own.base('p', own.start('p', 'set', 7))).toBe(2)
    expect(own.base('q', own.start('q', 'set', 1))).toBeUndefined()
  })

  it('видел строку непосчитанной — сверка «строки нет», а не с тем, что сейчас на экране', () => {
    const own = new OwnWrites()
    own.see('p', null)
    expect(own.baseOr('p', own.start('p', 'set', 3), 7)).toBeNull()
    expect(own.baseOr('q', own.start('q', 'set', 3), 7)).toBe(7)
  })

  it('«строки нет» и «посчитано 0» уходят на сервер по-разному', () => {
    expect(expectedBody(null)).toEqual({ expected_empty: true })
    expect(expectedBody(0)).toEqual({ expected_qty: 0 })
    expect(expectedQuery(null)).toBe('expected_empty=true')
    expect(expectedQuery(5)).toBe('expected_qty=5')
  })
})

describe('op_id ввода', () => {
  const ids = () => {
    let n = 0
    return () => `op-${++n}`
  }

  it('тот же не записанный ввод — тот же op_id, пока не записан; записан — следующий такой же новый', () => {
    const ops = new InputOps(ids())
    const first = ops.take('p', 'add|3')
    expect(ops.take('p', 'add|3')).toBe(first)
    ops.done('p', first)
    expect(ops.take('p', 'add|3')).not.toBe(first)
  })

  it('человек изменил ввод — новый op_id; другая строка — свой', () => {
    const ops = new InputOps(ids())
    const a = ops.take('p', 'add|3')
    const b = ops.take('p', 'add|4')
    expect(b).not.toBe(a)
    expect(ops.take('q', 'add|4')).not.toBe(b)
    expect(ops.take('p', 'add|4')).toBe(b)
  })

  it('записан старый ввод, а в поле уже новый — op_id нового не сбрасывается', () => {
    const ops = new InputOps(ids())
    const a = ops.take('p', 'add|3')
    const b = ops.take('p', 'add|4')
    ops.done('p', a)
    expect(ops.take('p', 'add|4')).toBe(b)
    ops.clear()
    expect(ops.take('p', 'add|4')).not.toBe(b)
  })
})

describe('свежесть показа', () => {
  const line = (id: string, counted: number | null, version: number) => ({ product_id: id, counted_qty: counted, version })

  it('перечитывание не возвращает строку, показанную новее; остальное — с сервера', () => {
    const shown = [line('a', 9, 12), line('b', 1, 3)]
    const incoming = [line('a', 7, 10), line('b', 2, 5)]
    expect(mergeLines(shown, incoming, 11)).toEqual([line('a', 9, 12), line('b', 2, 5)])
  })

  it('строка, которой нет в старом ответе, остаётся, если она новее ответа; убранная другим — уходит', () => {
    const shown = [line('a', 1, 20), line('b', 4, 3)]
    expect(mergeLines(shown, [], 15)).toEqual([line('a', 1, 20)])
  })

  it('ответ своей записи старее показанного — не показывается', () => {
    const l = line('a', 9, 12)
    expect(patchIfNewer(l, 10, (x) => ({ ...x, counted_qty: 7 }))).toBe(l)
    expect(patchIfNewer(l, 13, (x) => ({ ...x, counted_qty: 10 }))).toEqual({ ...l, counted_qty: 10, version: 13 })
    // Сервер без версий — показываем.
    expect(patchIfNewer(l, undefined, (x) => ({ ...x, counted_qty: 1 })).counted_qty).toBe(1)
  })
})

describe('количество в единицах найденного товара', () => {
  const piece = { unit: 'piece', container_ml: null }
  const can4 = { unit: 'ml', container_ml: 4000 }
  const can1 = { unit: 'ml', container_ml: 1000 }
  const bulk = { unit: 'ml', container_ml: null }
  const weight = { unit: 'g', container_ml: null }

  it('штуки — штуки; канистры — по таре найденного товара', () => {
    expect(qtyInUnits({ cans: 3, ml: 0 }, piece)).toBe(3)
    expect(qtyInUnits({ cans: 2, ml: 1500 }, can4)).toBe(9500)
    expect(qtyInUnits({ cans: 2, ml: 1500 }, can1)).toBe(3500)
    expect(qtyInUnits({ cans: 3, ml: 0 }, can4)).toBe(12_000)
    expect(qtyInUnits({ cans: 0, ml: 2500 }, bulk)).toBe(2500)
  })

  it('единицы несовместимы — не прибавлять молча', () => {
    expect(qtyInUnits({ cans: 1, ml: 500 }, piece)).toBeNull()
    expect(qtyInUnits({ cans: 2, ml: 0 }, bulk)).toBeNull()
    expect(qtyInUnits({ cans: 2, ml: 0 }, weight)).toBeNull()
  })

  it('кг — к кг, м — к м; кг к метрам и к штукам — нет (ADR-178)', () => {
    const meters = { unit: 'mm', container_ml: null }
    expect(qtyInUnits({ cans: 0, ml: 0, milli: { unit: 'g', qty: 2500 } }, weight)).toBe(2500)
    expect(qtyInUnits({ cans: 0, ml: 0, milli: { unit: 'mm', qty: 1250 } }, meters)).toBe(1250)
    expect(qtyInUnits({ cans: 0, ml: 0, milli: { unit: 'g', qty: 2500 } }, meters)).toBeNull()
    expect(qtyInUnits({ cans: 0, ml: 0, milli: { unit: 'mm', qty: 1250 } }, piece)).toBeNull()
    expect(qtyInUnits({ cans: 0, ml: 0, milli: { unit: 'mm', qty: 0 } }, piece)).toBe(0)
    expect(qtyInUnits({ cans: 2, ml: 0 }, meters)).toBeNull()
  })
})

describe('перечитывание после проведения', () => {
  const rev = (status: 'draft' | 'posted' | 'cancelled', counted: number, version: number) => ({
    head: { id: 'r', status },
    lines: [{ product_id: 'p', counted_qty: counted, version }],
    version,
  })

  it('ответ черновика, пришедший после «проведена», не возвращает экран в черновик', () => {
    const posted = rev('posted', 5, 3)
    expect(freshRevision(posted, rev('draft', 4, 2))).toBe(posted)
    const cancelled = rev('cancelled', 5, 3)
    expect(freshRevision(cancelled, rev('draft', 5, 3))).toBe(cancelled)
  })

  it('черновик провели — показывается проведённая; черновик поверх черновика — по версиям строк', () => {
    const posted = rev('posted', 5, 3)
    expect(freshRevision(rev('draft', 5, 3), posted)).toBe(posted)
    const merged = freshRevision(rev('draft', 6, 4), rev('draft', 5, 3))
    expect(merged.lines[0].counted_qty).toBe(6)
    expect(freshRevision(null, posted)).toBe(posted)
  })

  it('другая ревизия — как пришла', () => {
    const other = { ...rev('draft', 1, 1), head: { id: 'x', status: 'draft' as const } }
    expect(freshRevision(rev('posted', 5, 3), other)).toBe(other)
  })
})

describe('прошлые нажатия без ответа', () => {
  it('ответа не было — нажатие в списке; другой ввод несёт его как прошлое; ответ пришёл — список пуст', () => {
    const lost = new LostOps()
    lost.lost('a')
    lost.lost('a')
    expect(lost.before('a')).toEqual([])
    lost.lost('b')
    expect(lost.before('c')).toEqual(['a', 'b'])
    expect(lost.before('b')).toEqual(['a'])
    lost.clear()
    expect(lost.before('c')).toEqual([])
  })
})

describe('ключ ввода поля', () => {
  it('«Прибавить» из окна «пока вы вводили» — тот же ключ, что у Enter в поле «+ добавить»', () => {
    let n = 0
    const ops = new InputOps(() => `op-${++n}`)
    const fromAsk = ops.take('p', inputKey('add', 20))
    // Ответ потерян, человек жмёт Enter в поле с тем же числом — тот же op_id.
    expect(ops.take('p', inputKey('add', 20))).toBe(fromAsk)
    expect(inputKey('set', null)).toBe('set|')
    expect(inputKey('add', 20)).not.toBe(inputKey('set', 20))
  })
})

describe('«Завести» товар не из программы', () => {
  const noAnswer = (e: unknown) => e instanceof Error && e.message === 'нет связи'
  it('прошлое нажатие завело карточку, исправление итога оборвалось — следующее «Завести» несёт те же нажатия, второй товар не заводится', async () => {
    let n = 0
    const ops = new InputOps(() => `op-${++n}`)
    const lost = new LostOps()
    const sent: { opId: string; prev: string[] }[] = []
    let created = false
    // Сервер: известное нажатие или прошлое из prev_op_ids — ответ «earlier», иначе — новая карточка.
    const known = new Set<string>()
    const send = async (opId: string, prev: string[]) => {
      sent.push({ opId, prev })
      if (opId === 'op-1') {
        created = true
        known.add(opId)
        throw new Error('нет связи')
      }
      if (prev.some((x) => known.has(x)) || known.has(opId)) return { earlier: true, counted: 5 }
      if (created) throw new Error('второй товар')
      return { earlier: false, counted: 0 }
    }
    const earlierOf = (r: { earlier: boolean; counted: number }) => (r.earlier ? { productId: 'p', counted: r.counted } : null)
    let fixFails = true
    const fixes: string[] = []
    const fix = async (_p: string, _expected: number, opId: string) => {
      fixes.push(opId)
      if (fixFails) throw new Error('нет связи')
    }
    const press = (count: number) => createCounted({ ops, lost, key: `body|${count}`, count, send, earlierOf, fix, noAnswer })
    await expect(press(5)).rejects.toThrow('нет связи')
    await expect(press(6)).rejects.toThrow('нет связи')
    fixFails = false
    await expect(press(6)).resolves.toEqual({ earlier: true, counted: 5 })
    expect(sent.map((s) => s.prev)).toEqual([[], ['op-1'], ['op-1']])
    expect(sent[2].opId).toBe(sent[1].opId)
    // Исправление итога — тем же op_id: дошло без ответа — второй раз не запишется.
    expect(fixes[1]).toBe(fixes[0])
    // Записано — следующее такое же нажатие уже новое.
    fixFails = true
    expect(ops.take('new', 'body|6')).not.toBe(sent[2].opId)
  })
})
