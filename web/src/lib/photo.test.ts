import { describe, expect, it } from 'vitest'
import { fitSize } from './photo'

describe('фото накладной', () => {
  it('длинная сторона — не больше 2000 точек, пропорции те же', () => {
    expect(fitSize(4000, 3000)).toEqual({ width: 2000, height: 1500 })
    expect(fitSize(3000, 4000)).toEqual({ width: 1500, height: 2000 })
    expect(fitSize(1200, 900)).toEqual({ width: 1200, height: 900 })
    expect(fitSize(0, 10)).toEqual({ width: 0, height: 0 })
  })
})
