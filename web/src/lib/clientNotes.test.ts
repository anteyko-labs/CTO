import { describe, expect, it } from 'vitest'
import { activeNotes, cashierNotesText, type ClientNote } from './clientNotes'

const note = (text: string, deleted = false): ClientNote => ({ id: text, text, created_at: '2026-10-10T05:00:00Z', deleted })

describe('пометки клиента', () => {
  it('на кассе — только действующие, одной строкой', () => {
    expect(cashierNotesText([note('стук справа'), note('старое', true), note(' заливать только 5W-30 ')])).toBe(
      'Пометки клиента: стук справа; заливать только 5W-30',
    )
  })
  it('нет пометок или все удалены — строки нет', () => {
    expect(cashierNotesText(undefined)).toBeNull()
    expect(cashierNotesText([note('x', true)])).toBeNull()
    expect(activeNotes([note('a'), note('b', true)]).map((n) => n.id)).toEqual(['a'])
  })
})
