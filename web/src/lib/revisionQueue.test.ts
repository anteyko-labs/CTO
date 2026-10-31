// Неотправленные сканы ревизии на устройстве (задача 003, Н14): скан без ответа сервера не
// пропадает молча — остаётся в очереди со своим op_id и уходит при связи.
import { describe, expect, it } from 'vitest'
import { ApiError } from './api'
import {
  queueAdd,
  queueAfterSend,
  queueKey,
  queueToSend,
  readQueue,
  sendOutcome,
  unsentByLine,
  unsentCount,
  unsentText,
  writeQueue,
  type QueuedScan,
} from './revisionQueue'

/** Память вместо localStorage. */
function memory(): Pick<Storage, 'getItem' | 'setItem' | 'removeItem'> & { data: Map<string, string> } {
  const data = new Map<string, string>()
  return {
    data,
    getItem: (k) => data.get(k) ?? null,
    setItem: (k, v) => void data.set(k, v),
    removeItem: (k) => void data.delete(k),
  }
}

const scan = (opId: string, productId = 'p', qty = 1): QueuedScan => ({ opId, productId, qty, unsent: false })

describe('очередь неотправленных сканов', () => {
  it('скан без ответа остаётся со своим op_id; записанный и отклонённый — уходят', () => {
    let q = queueAdd([], scan('a'))
    q = queueAdd(q, scan('b'))
    q = queueAdd(q, scan('c'))
    // Тот же скан второй раз не встаёт.
    q = queueAdd(q, scan('a'))
    expect(q.map((x) => x.opId)).toEqual(['a', 'b', 'c'])
    q = queueAfterSend(q, 'a', 'no_answer')
    q = queueAfterSend(q, 'b', 'ok')
    q = queueAfterSend(q, 'c', 'refused')
    expect(q).toEqual([{ opId: 'a', productId: 'p', qty: 1, unsent: true }])
  })

  it('отметка строки: сумма неотправленного, следующий скан её не стирает', () => {
    let q = queueAdd([], scan('a', 'p', 1))
    q = queueAfterSend(q, 'a', 'no_answer')
    q = queueAdd(q, scan('b', 'p', 1))
    q = queueAfterSend(q, 'b', 'no_answer')
    q = queueAdd(q, scan('c', 'oil', 4000))
    q = queueAfterSend(q, 'c', 'no_answer')
    // Новый скан ещё идёт — в отметке его нет, неотправленные — на месте.
    q = queueAdd(q, scan('d', 'p', 1))
    expect(unsentByLine(q)).toEqual(new Map([['p', 2], ['oil', 4000]]))
    expect(unsentCount(q)).toBe(3)
    expect(unsentText(3)).toBe('Не отправлено: 3 — отправится при связи')
  })

  it('переживает перезагрузку: после чтения всё — неотправленное, тот же op_id', () => {
    const store = memory()
    let q = queueAdd([], scan('a'))
    q = queueAfterSend(q, 'a', 'no_answer')
    q = queueAdd(q, scan('b', 'x', 3))
    writeQueue(store, 'rev1', q)
    expect(readQueue(store, 'rev1')).toEqual([
      { opId: 'a', productId: 'p', qty: 1, unsent: true },
      { opId: 'b', productId: 'x', qty: 3, unsent: true },
    ])
    // Очередь — своя у каждой ревизии.
    expect(readQueue(store, 'rev2')).toEqual([])
    // Всё отправлено — ключ убирается.
    writeQueue(store, 'rev1', [])
    expect(store.data.has(queueKey('rev1'))).toBe(false)
  })

  it('испорченная запись в хранилище — пустая очередь, а не ошибка экрана', () => {
    const store = memory()
    store.setItem(queueKey('r'), '{не json')
    expect(readQueue(store, 'r')).toEqual([])
    store.setItem(queueKey('r'), JSON.stringify([{ opId: 'a', productId: 'p', qty: 'один' }, { opId: 'b', productId: 'p', qty: 1 }]))
    expect(readQueue(store, 'r')).toEqual([{ opId: 'b', productId: 'p', qty: 1, unsent: true }])
  })

  it('при связи отправляется неотправленное по порядку, кроме того, что уже идёт', () => {
    let q = queueAdd([], scan('a'))
    q = queueAfterSend(q, 'a', 'no_answer')
    q = queueAdd(q, scan('b'))
    q = queueAfterSend(q, 'b', 'no_answer')
    // Свежий скан отправляется сам — очередь его не дублирует.
    q = queueAdd(q, scan('c'))
    expect(queueToSend(q, new Set(['b'])).map((x) => x.opId)).toEqual(['a'])
  })

  it('ответа нет — ждать связи; ответ сервера с отказом — не повторять', () => {
    expect(sendOutcome(new ApiError(0, 'network', 'Нет связи с сервером'))).toBe('no_answer')
    expect(sendOutcome(new ApiError(504, 'http', 'Сервер недоступен'))).toBe('no_answer')
    expect(sendOutcome(new ApiError(409, 'revision_closed', 'Ревизия уже проведена'))).toBe('refused')
    expect(sendOutcome(new Error('что-то'))).toBe('refused')
  })
})
