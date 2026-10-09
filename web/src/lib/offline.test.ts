import { describe, expect, it } from 'vitest'
import { formatTempNumber, staleSending, type OutboxItem } from './offline'

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

  it('временный номер: хвост устройства и порядковый номер', () => {
    expect(formatTempNumber('0192a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b', '0042')).toBe('Ч-4A5B-0042')
  })
})
