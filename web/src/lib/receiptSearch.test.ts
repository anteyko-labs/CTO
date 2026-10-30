import { describe, expect, it } from 'vitest'
import { receiptsQuery } from './receiptSearch'
import { qs } from './api'

const base = { from: '2026-09-30', to: '2026-10-30', kind: 'all', q: '', photo: false }

describe('поиск приходов', () => {
  it('без поиска — период', () => {
    expect(qs(receiptsQuery(base))).toBe('?from=2026-09-30&to=2026-10-30&kind=all')
  })

  it('с поиском — за всё время', () => {
    expect(receiptsQuery({ ...base, q: '  НК-777 ' })).toEqual({ q: 'НК-777', kind: 'all', photo: undefined })
  })

  it('отбор «с фото»', () => {
    expect(qs(receiptsQuery({ ...base, photo: true }))).toContain('photo=true')
    expect(qs(receiptsQuery(base))).not.toContain('photo')
  })
})
