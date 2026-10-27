import { describe, expect, it } from 'vitest'
import { ApiError } from './api'
import {
  afterRejection,
  expiredDone,
  foreignWaiting,
  formatTempNumber,
  isOwnItem,
  keepInQueue,
  logoutWarning,
  lookupOffline,
  pendingCount,
  queuedBody,
  receiptOf,
  receiptsWord,
  retryDelayMs,
  searchSnapshotClients,
  sendOutcome,
  snapshotIsStale,
  snapshotMeta,
  snapshotStep,
  staleSending,
  subtractStock,
  waitsRetry,
  type OutboxItem,
  type Snapshot,
} from './offline'
import type { Sale } from './types'

const item = (op_id: string, status: OutboxItem['status']): OutboxItem => ({
  op_id,
  temp_no: '',
  created_at: '2026-10-08T00:00:00Z',
  status,
  attempts: 0,
  last_error: '',
  body: {},
})

describe('очередь чеков', () => {
  it('после перезапуска «отправляется» снова ждёт отправки, остальное не трогается', () => {
    const back = staleSending([item('a', 'sending'), item('b', 'pending'), item('c', 'done'), item('d', 'rejected')])
    expect(back).toEqual([{ ...item('a', 'sending'), status: 'pending' }])
  })

  it('неотправленные перед закрытием смены — ждущие и отправляемые, но не принятые и не отклонённые', () => {
    expect(pendingCount([item('a', 'sending'), item('b', 'pending'), item('c', 'done'), item('d', 'rejected')])).toBe(2)
  })

  it('временный номер: хвост устройства и порядковый номер', () => {
    expect(formatTempNumber('0192a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b', '0042')).toBe('Ч-4A5B-0042')
  })
})

describe('ошибки отправки чека (ADR-062)', () => {
  const err = (status: number, code = 'error') => new ApiError(status, code, 'отказ')

  it('сбой сервера, обрыв и истёкший вход чек не удаляют', () => {
    expect(keepInQueue(err(500))).toBe(true)
    expect(keepInQueue(err(503))).toBe(true)
    expect(keepInQueue(err(0, 'network'))).toBe(true)
    expect(keepInQueue(err(401))).toBe(true)
    expect(keepInQueue(new Error('что-то'))).toBe(true)
    expect(keepInQueue(err(422))).toBe(false)
    expect(keepInQueue(err(409))).toBe(false)
  })

  it('скан при 502 прокси и 5xx сервера ищет товар в снимке, 404 — «не найден»', () => {
    expect(lookupOffline(err(502))).toBe(true)
    expect(lookupOffline(err(500))).toBe(true)
    expect(lookupOffline(err(0, 'network'))).toBe(true)
    expect(lookupOffline(new Error('что-то'))).toBe(true)
    expect(lookupOffline(err(404))).toBe(false)
    expect(lookupOffline(err(401))).toBe(false)
  })

  it('отказ по существу — владельцу; 500 — ждать; 409 — повторить несколько раз', () => {
    expect(sendOutcome(err(422), 0)).toBe('reject')
    expect(sendOutcome(err(500), 0)).toBe('keep')
    expect(sendOutcome(err(401), 0)).toBe('keep')
    expect(sendOutcome(err(409), 0)).toBe('conflict_retry')
    expect(sendOutcome(err(409), 4)).toBe('reject')
  })

  it('«отклонён» — только когда сервер принял отчёт об отказе', () => {
    const it0 = { ...item('a', 'sending'), attempts: 2 }
    expect(afterRejection(it0, 'кассир не найден', true)).toMatchObject({ status: 'rejected', last_error: 'кассир не найден' })
    const back = afterRejection(it0, 'кассир не найден', false)
    expect(back.status).toBe('pending')
    expect(back.attempts).toBe(3)
    expect(back.last_error).toContain('кассир не найден')
  })
})

describe('очередь по пользователям и повторы (ADR-093)', () => {
  const mine = { ...item('a', 'pending'), user_id: 'u1', user_name: 'Админ' }
  const theirs = { ...item('b', 'pending'), user_id: 'u2', user_name: 'Эрбол' }
  const theirs2 = { ...item('c', 'sending'), user_id: 'u2', user_name: 'Эрбол' }

  it('свои чеки и старые без пользователя — свои; чужие ждут входа хозяина', () => {
    expect(isOwnItem(mine, 'u1')).toBe(true)
    expect(isOwnItem(item('x', 'pending'), 'u1')).toBe(true)
    expect(isOwnItem(theirs, 'u1')).toBe(false)
    expect(isOwnItem({ ...theirs, claimed_by: 'u1' }, 'u1')).toBe(true)
    expect(foreignWaiting([mine, theirs, theirs2, { ...item('d', 'done'), user_id: 'u2' }], 'u1')).toEqual([
      { user_id: 'u2', user_name: 'Эрбол', count: 2 },
    ])
  })

  it('чужой чек, взятый владельцем, уходит с отметкой, чей он; всегда — без сети', () => {
    expect(queuedBody({ ...theirs, body: { op_id: 'b' }, claimed_by: 'owner' })).toEqual({ op_id: 'b', offline: true, claimed_user_id: 'u2' })
    expect(queuedBody({ ...mine, body: { op_id: 'a' } })).toEqual({ op_id: 'a', offline: true })
  })

  it('пауза после сбоя: 30 с → 1 мин → 5 мин', () => {
    expect([1, 2, 3, 9].map(retryDelayMs)).toEqual([30_000, 60_000, 300_000, 300_000])
    const now = Date.parse('2026-10-20T10:00:00Z')
    const back = afterRejection(mine, 'нет', false, now)
    expect(back.next_try_at).toBe('2026-10-20T10:00:30.000Z')
    expect(waitsRetry(back, now)).toBe(true)
    expect(waitsRetry(back, now + 31_000)).toBe(false)
    expect(afterRejection(mine, 'нет', true, now).next_try_at).toBeUndefined()
  })

  it('отправленные хранятся неделю', () => {
    const now = Date.parse('2026-10-20T00:00:00Z')
    const old = { ...item('o', 'done'), created_at: '2026-10-12T00:00:00Z' }
    const fresh = { ...item('f', 'done'), created_at: '2026-10-15T00:00:00Z' }
    const oldRejected = { ...item('r', 'rejected'), created_at: '2026-09-01T00:00:00Z' }
    expect(expiredDone([old, fresh, oldRejected], now)).toEqual([old])
  })

  it('чеки словом и предупреждение перед выходом', () => {
    expect([1, 2, 5, 11, 21, 22].map(receiptsWord)).toEqual(['чек', 'чека', 'чеков', 'чеков', 'чек', 'чека'])
    expect(logoutWarning([mine, theirs], 'u1', true)).toContain('1 чек ещё не отправлен')
    expect(logoutWarning([theirs], 'u1', true)).toBeNull()
    expect(logoutWarning([], 'u1', false)).toContain('Нет связи')
  })

  it('печать из очереди: номер сервера, если он уже есть', () => {
    const receipt = { id: '', number: 'Ч-1', total_tyiyn: 100 } as unknown as Sale
    expect(receiptOf({ ...mine, temp_no: 'Ч-1', receipt })?.number).toBe('Ч-1')
    expect(receiptOf({ ...mine, temp_no: 'Ч-1', receipt, server_number: 42 })?.number).toBe(42)
    expect(receiptOf(mine)).toBeNull()
  })
})

describe('снимок кассы без сети', () => {
  const snap = {
    version: '1',
    server_time: '',
    saved_at: '',
    oil_change_master_fee_tyiyn: 0,
    employees: [],
    products: [{ id: 'p1', stock_qty: 8000 }, { id: 'p2', stock_qty: 3 }],
    clients: [
      { id: 'c1', name: 'Улан Исаков', phone: '+996 555 12-34-56', inn: '', contacts: [], vehicles: [{ id: 'v1', plate: '01KG 093 AAA' }] },
      { id: 'c2', name: 'ОсОО Транс', phone: '', inn: '01234567890123', contacts: [], vehicles: [] },
    ],
  } as unknown as Snapshot

  it('свой чек без сети уменьшает остаток в снимке', () => {
    const next = subtractStock(snap, [
      { product_id: 'p1', units: 4000 },
      { product_id: 'p1', units: 1000 },
      { product_id: 'p2', units: 5 },
    ])
    expect(next.products.map((p) => p.stock_qty)).toEqual([3000, -2])
  })

  it('клиент находится по имени, телефону, ИНН и госномеру', () => {
    expect(searchSnapshotClients(snap, 'исаков').map((c) => c.id)).toEqual(['c1'])
    expect(searchSnapshotClients(snap, '5551234').map((c) => c.id)).toEqual(['c1'])
    expect(searchSnapshotClients(snap, '012345678').map((c) => c.id)).toEqual(['c2'])
    expect(searchSnapshotClients(snap, '093aaa').map((c) => c.id)).toEqual(['c1'])
    expect(searchSnapshotClients(null, 'исаков')).toEqual([])
  })
})

describe('снимок без лишних чтений (SPEC-36 §16)', () => {
  it('304 — только время; новый снимок — сразу при пустом чеке, иначе ждёт пустого', () => {
    expect(snapshotStep(304, true, false)).toBe('touch')
    expect(snapshotStep(304, true, true)).toBe('touch')
    expect(snapshotStep(200, true, true)).toBe('save')
    expect(snapshotStep(200, true, false)).toBe('later')
  })
  it('снимка ещё нет — сохраняется и при непустом чеке; ошибка сервера — пропуск', () => {
    expect(snapshotStep(200, false, false)).toBe('save')
    expect(snapshotStep(304, false, true)).toBe('skip')
    expect(snapshotStep(500, true, true)).toBe('skip')
    expect(snapshotStep(401, false, true)).toBe('skip')
  })
  it('время и версия снимка — отдельно; «устарел» считается по ним', () => {
    const snap = { version: 'v1', saved_at: new Date().toISOString(), products: [] } as unknown as Snapshot
    expect(snapshotMeta(snap)).toEqual({ version: 'v1', saved_at: snap.saved_at })
    expect(snapshotIsStale(snapshotMeta(snap))).toBe(false)
    expect(snapshotIsStale({ saved_at: new Date(Date.now() - 3 * 3600_000).toISOString() })).toBe(true)
    expect(snapshotIsStale(null)).toBe(true)
  })
})
