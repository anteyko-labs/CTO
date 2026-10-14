import { describe, expect, it } from 'vitest'
import { ApiError } from './api'
import { afterRejection, formatTempNumber, keepInQueue, pendingCount, sendOutcome, staleSending, type OutboxItem } from './offline'

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
