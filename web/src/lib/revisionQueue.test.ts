// Неотправленные сканы ревизии на устройстве (задача 003, Н14): скан без ответа сервера не
// пропадает молча — остаётся в очереди со своим op_id и уходит при связи.
import { describe, expect, it } from 'vitest'
import { ApiError } from './api'
import {
  cleanQueues,
  closedText,
  newScan,
  otherQueues,
  QUEUE_MAX_AGE_MS,
  queueAdd,
  queueKey,
  queueResolve,
  queueToSend,
  queueWithout,
  queueWriter,
  QUEUE_LOCK,
  readQueue,
  scansText,
  sendOutcome,
  unsentByLine,
  unsentCodes,
  unsentCount,
  unsentFirstText,
  unsentText,
  updateQueue,
  withQueueLock,
  type QueueLocks,
  type QueuedScan,
} from './revisionQueue'

/** Память вместо localStorage. */
function memory() {
  const data = new Map<string, string>()
  return {
    data,
    get length() {
      return data.size
    },
    key: (i: number) => [...data.keys()][i] ?? null,
    getItem: (k: string) => data.get(k) ?? null,
    setItem: (k: string, v: string) => void data.set(k, v),
    removeItem: (k: string) => void data.delete(k),
  }
}

const NOW = 1_800_000_000_000
const scan = (opId: string, productId = 'p', qty = 1, at = NOW): QueuedScan => newScan(opId, { productId, qty }, at)
const byCode = (opId: string, barcode: string, at = NOW): QueuedScan => newScan(opId, { barcode }, at)
const AIBEK = { id: 'u1', name: 'Айбек' }
const NURLAN = { id: 'u2', name: 'Нурлан' }
const none = new Set<string>()

/** Блокировка как `navigator.locks`: одна на имя, по очереди запросов, выдаётся не сразу. */
function fakeLocks() {
  const waiting: (() => void)[] = []
  let busy = false
  const names: string[] = []
  const next = () => {
    const go = waiting.shift()
    if (go) go()
    else busy = false
  }
  const locks: QueueLocks & { held: () => boolean; names: string[] } = {
    names,
    held: () => busy,
    request(name, callback) {
      names.push(name)
      return new Promise((resolve, reject) => {
        const go = () => {
          busy = true
          // Как в браузере: колбэк — в отдельной задаче, а не сразу при запросе.
          void Promise.resolve()
            .then(callback)
            .then(resolve, reject)
            .finally(next)
        }
        if (busy) waiting.push(go)
        else {
          busy = true
          setTimeout(go, 0)
        }
      })
    },
  }
  return locks
}

describe('очередь неотправленных сканов', () => {
  it('тот же скан второй раз не встаёт; убираются только свои отправленные', () => {
    let q = queueAdd([], scan('a'))
    q = queueAdd(q, scan('b'))
    q = queueAdd(q, scan('a'))
    expect(q.map((x) => x.opId)).toEqual(['a', 'b'])
    expect(queueWithout(q, ['b', 'чужой']).map((x) => x.opId)).toEqual(['a'])
  })

  it('две вкладки одного устройства: запись — слияние со свежей очередью, а не «последний прав»', () => {
    const store = memory()
    // Вкладка 1 и вкладка 2 открыли ревизию с пустой очередью.
    let tab1: QueuedScan[] = readQueue(store, 'rev', AIBEK.id) ?? []
    let tab2: QueuedScan[] = readQueue(store, 'rev', AIBEK.id) ?? []
    tab1 = updateQueue(store, 'rev', AIBEK, (q) => queueAdd(q, scan('a')), tab1)
    // Вкладка 2 ещё не перечитала — её запись всё равно не затирает скан вкладки 1.
    tab2 = updateQueue(store, 'rev', AIBEK, (q) => queueAdd(q, scan('b')), tab2)
    expect(readQueue(store, 'rev', AIBEK.id)?.map((x) => x.opId)).toEqual(['a', 'b'])
    expect(tab2.map((x) => x.opId)).toEqual(['a', 'b'])
    // Вкладка 1 отправила свой — убирается только он, скан вкладки 2 остаётся.
    tab1 = updateQueue(store, 'rev', AIBEK, (q) => queueWithout(q, ['a']), tab1)
    expect(readQueue(store, 'rev', AIBEK.id)?.map((x) => x.opId)).toEqual(['b'])
    tab2 = updateQueue(store, 'rev', AIBEK, (q) => queueWithout(q, ['b']), tab2)
    // Всё отправлено — ключ убирается.
    expect(store.data.has(queueKey('rev', AIBEK.id))).toBe(false)
    expect(tab1).toEqual([{ ...scan('b') }])
    expect(tab2).toEqual([])
  })

  it('хранилище закрыто — очередь живёт в памяти вкладки', () => {
    const broken = {
      getItem: () => {
        throw new Error('закрыто')
      },
      setItem: () => {
        throw new Error('закрыто')
      },
      removeItem: () => {},
    }
    expect(readQueue(broken, 'rev', 'u')).toBeNull()
    const q = updateQueue(broken, 'rev', AIBEK, (x) => queueAdd(x, scan('b')), [scan('a')])
    expect(q.map((x) => x.opId)).toEqual(['a', 'b'])
  })

  it('ключ — ревизия и пользователь: сканы одного не уходят под другим, видны как чужие', () => {
    const store = memory()
    expect(queueKey('rev', 'u1')).not.toBe(queueKey('rev', 'u2'))
    updateQueue(store, 'rev', AIBEK, (q) => queueAdd(queueAdd(q, scan('a')), scan('b')), [])
    updateQueue(store, 'rev2', NURLAN, (q) => queueAdd(q, scan('c')), [])
    expect(readQueue(store, 'rev', NURLAN.id)).toEqual([])
    expect(otherQueues(store, 'rev', NURLAN.id)).toEqual([{ user: 'Айбек', count: 2 }])
    expect(otherQueues(store, 'rev', AIBEK.id)).toEqual([])
  })

  it('отметка строки и вверху: сумма неотправленного; свежий скан, который ещё идёт, — не в ней', () => {
    let q = queueAdd([], scan('a', 'p', 1))
    q = queueAdd(q, scan('b', 'p', 1))
    q = queueAdd(q, scan('c', 'oil', 4000))
    q = queueAdd(q, byCode('e', '4600001'))
    q = queueAdd(q, scan('d', 'p', 1))
    const fresh = new Set(['d'])
    expect(unsentByLine(q, fresh)).toEqual(new Map([['p', 2], ['oil', 4000]]))
    expect(unsentCodes(q, fresh)).toEqual(['4600001'])
    expect(unsentCount(q, fresh)).toBe(4)
    expect(unsentText(4)).toBe('Не отправлено: 4 — отправится при связи')
    expect(queueToSend(q, new Set(['a'])).map((x) => x.opId)).toEqual(['b', 'c', 'e', 'd'])
  })

  it('скан по коду: товар нашёлся — тот же op_id, с товаром и прибавкой', () => {
    const q = queueResolve([byCode('e', '4600001'), scan('a')], 'e', 'p9', 1)
    expect(q[0]).toEqual({ opId: 'e', productId: 'p9', barcode: null, qty: 1, at: NOW })
    expect(byCode('e', '4600001')).toEqual({ opId: 'e', productId: null, barcode: '4600001', qty: 0, at: NOW })
    expect(unsentByLine(q, none).get('p9')).toBe(1)
  })

  it('переживает перезагрузку; испорченная запись — пустая очередь, а не ошибка экрана', () => {
    const store = memory()
    updateQueue(store, 'r', AIBEK, (q) => queueAdd(queueAdd(q, scan('a')), byCode('b', '777')), [])
    expect(readQueue(store, 'r', AIBEK.id)).toEqual([scan('a'), byCode('b', '777')])
    store.setItem(queueKey('r', 'u'), '{не json')
    expect(readQueue(store, 'r', 'u')).toEqual([])
    store.setItem(queueKey('r', 'u'), JSON.stringify({ user: 'x', items: [{ opId: 'a', productId: 'p', qty: 'один', at: 1 }, scan('b')] }))
    expect(readQueue(store, 'r', 'u')).toEqual([scan('b')])
  })

  it('чистка: ревизия проведена — очередь убрана, сколько не учтено — сказано; старше недели — убраны', () => {
    const store = memory()
    updateQueue(store, 'posted', AIBEK, (q) => queueAdd(queueAdd(q, scan('a')), scan('b')), [])
    updateQueue(store, 'posted', NURLAN, (q) => queueAdd(q, scan('c')), [])
    updateQueue(store, 'draft', AIBEK, (q) => queueAdd(queueAdd(q, scan('old', 'p', 1, NOW - QUEUE_MAX_AGE_MS - 1)), scan('new')), [])
    updateQueue(store, 'stale', AIBEK, (q) => queueAdd(q, scan('old2', 'p', 1, NOW - QUEUE_MAX_AGE_MS - 1)), [])
    store.setItem('avtodom.revision.unsent.legacy', '[]')
    store.setItem('avtodom.other', 'не трогать')
    const lost = cleanQueues(store, new Map([['posted', { number: 7, status: 'posted' as const }]]), NOW)
    expect(lost).toEqual([{ revisionId: 'posted', number: 7, status: 'posted', count: 3 }])
    expect([...store.data.keys()].sort()).toEqual(['avtodom.other', queueKey('draft', AIBEK.id)])
    expect(readQueue(store, 'draft', AIBEK.id)?.map((x) => x.opId)).toEqual(['new'])
    expect(closedText(3, 7, 'posted')).toBe('3 скана не учтены: ревизия № 7 проведена')
    expect(closedText(1, 7, 'cancelled')).toBe('1 скан не учтён: ревизия № 7 отменена')
  })

  it('в очереди остаётся всё, кроме окончательного отказа по существу', () => {
    const wait = [
      new ApiError(0, 'network', 'Нет связи с сервером'),
      new ApiError(504, 'error', 'Сервер недоступен'),
      new ApiError(500, 'internal', 'Внутренняя ошибка'),
      new ApiError(429, 'too_many_requests', 'Слишком часто'),
      new ApiError(404, 'error', 'Ошибка 404'),
      new ApiError(200, 'bad_response', 'Непонятный ответ'),
      new Error('что-то'),
    ]
    for (const e of wait) expect(sendOutcome(e), e.message).toBe('wait')
    expect(sendOutcome(new ApiError(401, 'unauthorized', 'Войдите'))).toBe('login')
    expect(sendOutcome(new ApiError(409, 'revision_closed', 'Ревизия уже проведена'))).toBe('refused')
    expect(sendOutcome(new ApiError(409, 'revision_overlap', 'Пересекается'))).toBe('refused')
    expect(sendOutcome(new ApiError(422, 'validation', 'Неверные данные'))).toBe('refused')
  })

  it('тексты: «Сначала отправятся N неотправленных сканов», число сканов', () => {
    expect(scansText(1)).toBe('1 скан')
    expect(scansText(2)).toBe('2 скана')
    expect(scansText(5)).toBe('5 сканов')
    expect(unsentFirstText(1)).toMatch(/^Сначала отправится 1 неотправленный скан этой строки/)
    expect(unsentFirstText(3)).toMatch(/^Сначала отправятся 3 неотправленных скана этой строки/)
  })

  it('две вкладки под блокировкой: хранилище меняется только под ней, сканы обеих вкладок целы', async () => {
    const mem = memory()
    const locks = fakeLocks()
    // Хранилище, которое помнит, писали ли в него без блокировки.
    let outside = 0
    const store = {
      ...mem,
      getItem: (k: string) => {
        if (!locks.held()) outside++
        return mem.getItem(k)
      },
      setItem: (k: string, v: string) => {
        if (!locks.held()) outside++
        mem.setItem(k, v)
      },
      removeItem: (k: string) => {
        if (!locks.held()) outside++
        mem.removeItem(k)
      },
    }
    const shown1: string[][] = []
    const tab1 = queueWriter(store, 'rev', AIBEK, locks, (q) => shown1.push(q.map((x) => x.opId)))
    const tab2 = queueWriter(store, 'rev', AIBEK, locks, () => {})
    outside = 0 // открытие экрана читает без блокировки — это не запись
    const w1 = tab1.put((q) => queueAdd(q, scan('a')))
    const w2 = tab2.put((q) => queueAdd(q, scan('b')))
    const w3 = tab1.put((q) => queueAdd(q, scan('c')))
    // На экране — сразу, хотя блокировка ещё не далась.
    expect(tab1.current.map((x) => x.opId)).toEqual(['a', 'c'])
    expect(mem.data.size).toBe(0)
    await Promise.all([w1, w2, w3])
    expect(outside).toBe(0)
    expect(locks.names).toEqual([QUEUE_LOCK, QUEUE_LOCK, QUEUE_LOCK])
    expect(readQueue(mem, 'rev', AIBEK.id)?.map((x) => x.opId)).toEqual(['a', 'b', 'c'])
    // Запись первой вкладки дошла раньше скана «c» — на экране он не пропадал.
    expect(shown1.slice(1).every((ids) => ids.includes('a') && ids.includes('c'))).toBe(true)
    // Вкладка 1 узнала о скане вкладки 2 по событию `storage`.
    tab1.reload()
    expect(tab1.current.map((x) => x.opId)).toEqual(['a', 'b', 'c'])
    // Отправила свои — остался только чужой.
    await tab1.put((q) => queueWithout(q, ['a', 'c']))
    expect(readQueue(mem, 'rev', AIBEK.id)?.map((x) => x.opId)).toEqual(['b'])
  })

  it('перечитывание при ждущей записи: своё изменение остаётся поверх', async () => {
    const store = memory()
    const locks = fakeLocks()
    const tab = queueWriter(store, 'rev', AIBEK, locks, () => {})
    const w = tab.put((q) => queueAdd(q, scan('a')))
    updateQueue(store, 'rev', AIBEK, (q) => queueAdd(q, scan('b')), []) // другая вкладка
    tab.reload()
    expect(tab.current.map((x) => x.opId)).toEqual(['b', 'a'])
    await w
    expect(tab.current.map((x) => x.opId)).toEqual(['b', 'a'])
    expect(readQueue(store, 'rev', AIBEK.id)?.map((x) => x.opId)).toEqual(['b', 'a'])
  })

  it('блокировки нет или она не далась — пишется сразу, один раз', async () => {
    const store = memory()
    const tab = queueWriter(store, 'rev', AIBEK, null, () => {})
    void tab.put((q) => queueAdd(q, scan('a')))
    expect(readQueue(store, 'rev', AIBEK.id)?.map((x) => x.opId)).toEqual(['a'])
    let runs = 0
    const broken: QueueLocks = { request: () => Promise.reject(new Error('нет блокировок')) }
    expect(await withQueueLock(broken, () => ++runs)).toBe(1)
    expect(runs).toBe(1)
    // Ошибка самой записи не повторяет её.
    const fine = fakeLocks()
    await expect(
      withQueueLock(fine, () => {
        runs++
        throw new Error('сломалось')
      }),
    ).rejects.toThrow('сломалось')
    expect(runs).toBe(2)
    expect(await withQueueLock(fine, () => 'чистка')).toBe('чистка')
  })
})
