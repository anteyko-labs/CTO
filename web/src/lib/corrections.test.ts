import { describe, expect, it } from 'vitest'
import { correctionMarks, numberText, type Correction } from './corrections'

const fix = (original: string, originalNo: number | null, next: string, nextNo: number | null): Correction => ({
  id: `c-${original}`,
  kind: 'sale',
  kind_label: 'чек',
  original_id: original,
  original_number: originalNo,
  reversal_id: `r-${original}`,
  new_id: next,
  new_number: nextNo,
  comment: 'ошибка',
  diff: [],
  user_name: 'Владелец',
  created_at: '2026-10-19T10:00:00Z',
})

describe('отметки исправлений в списках', () => {
  it('исходный — «исправлен → № N», новый — «исправление № M»; цепочка исправлений', () => {
    const marks = correctionMarks([fix('a', 10, 'b', 12), fix('b', 12, 'c', 15)])
    expect(marks.get('a')?.correctedTo?.new_number).toBe(12)
    expect(marks.get('a')?.correctionOf).toBeNull()
    expect(marks.get('b')?.correctionOf?.original_number).toBe(10)
    expect(marks.get('b')?.correctedTo?.new_number).toBe(15)
    expect(marks.get('c')?.correctionOf?.original_number).toBe(12)
    expect(marks.get('c')?.correctedTo).toBeNull()
    expect(marks.get('x')).toBeUndefined()
  })

  it('документ без номера', () => {
    const marks = correctionMarks([fix('a', null, 'b', null)])
    expect(numberText(marks.get('a')?.correctedTo?.new_number ?? null)).toBe('')
    expect(numberText(marks.get('b')?.correctionOf?.original_number ?? null)).toBe('')
    expect(numberText(7)).toBe('№ 7')
    expect(numberText(null)).toBe('')
  })
})
