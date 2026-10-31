import { describe, expect, it } from 'vitest'
import { branchByRow, branchParam, rowsOf } from './branchReport'

describe('«Все филиалы»', () => {
  it('параметр запроса и строки ответа', () => {
    expect(branchParam(true)).toEqual({ branch: 'all' })
    expect(branchParam(false)).toEqual({})
    expect(rowsOf([1, 2])).toEqual([1, 2])
    expect(rowsOf({ items: [3] })).toEqual([3])
  })

  it('филиал каждой строки — по разбивке (вкладка «Оклад», задача 003)', () => {
    const parts = [
      { branch_id: 'a', name: 'Центр', archived: false, report: { rows: [{ employee_id: 'e1' }, { employee_id: 'e2' }] } },
      { branch_id: 'b', name: 'Восток', archived: false, report: { rows: [{ employee_id: 'e3' }] } },
    ]
    const m = branchByRow(parts, (r) => r.rows, (x) => x.employee_id)
    expect(m.get('e1')).toBe('Центр')
    expect(m.get('e3')).toBe('Восток')
    expect(m.size).toBe(3)
    expect(branchByRow(undefined, (r: { rows: { id: string }[] }) => r.rows, (x) => x.id).size).toBe(0)
  })
})
