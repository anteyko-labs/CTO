import { describe, expect, it } from 'vitest'
import { correctionMarks, markText, numberText, type Correction } from './corrections'

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
    expect(markText(marks.get('a'))).toEqual(['исправлен → № 12'])
    expect(markText(marks.get('b'))).toEqual(['исправление № 10', 'исправлен → № 15'])
    expect(markText(marks.get('c'))).toEqual(['исправление № 12'])
    expect(markText(marks.get('x'))).toEqual([])
  })

  it('документ без номера', () => {
    const marks = correctionMarks([fix('a', null, 'b', null)])
    expect(markText(marks.get('a'))).toEqual(['исправлен → новый'])
    expect(markText(marks.get('b'))).toEqual(['исправление'])
    expect(numberText(7)).toBe('№ 7')
    expect(numberText(null)).toBe('')
  })
})
