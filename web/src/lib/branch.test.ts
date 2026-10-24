import { describe, expect, it } from 'vitest'
import { branchHeaders, rememberBranch, selectedBranch } from './branch'
import { branchParam, rowsOf } from './branchReport'

describe('филиал запроса (SPEC-33)', () => {
  it('выбранный филиал уходит заголовком, запись очереди — в свой филиал', () => {
    rememberBranch(null)
    expect(branchHeaders()).toEqual({})
    rememberBranch('b-1')
    expect(selectedBranch()).toBe('b-1')
    expect(branchHeaders()).toEqual({ 'X-Branch-Id': 'b-1' })
    // Чек без сети, пробитый в другом филиале, уходит туда, где его пробили.
    expect(branchHeaders('b-2')).toEqual({ 'X-Branch-Id': 'b-2' })
    // Запись очереди своего филиала пользователя — без заголовка.
    expect(branchHeaders(null)).toEqual({})
    rememberBranch(null)
  })

  it('«Все филиалы» — параметр отчёта и список строк из ответа', () => {
    expect(branchParam(true)).toEqual({ branch: 'all' })
    expect(branchParam(false)).toEqual({})
    expect(rowsOf([1, 2])).toEqual([1, 2])
    expect(rowsOf({ items: [3] })).toEqual([3])
  })
})
