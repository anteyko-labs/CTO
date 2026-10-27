import { describe, expect, it } from 'vitest'
import { personPickLayout } from './cashierMode'

describe('мастер и кассир кнопками в режиме кассы (SPEC-36 §16)', () => {
  it('до трёх вариантов — кнопки в ряд, больше — окно с кнопками', () => {
    expect(personPickLayout(1)).toBe('inline')
    expect(personPickLayout(3)).toBe('inline')
    expect(personPickLayout(4)).toBe('window')
    expect(personPickLayout(40)).toBe('window')
  })
})
