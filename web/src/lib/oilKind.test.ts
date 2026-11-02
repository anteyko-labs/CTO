import { describe, expect, it } from 'vitest'
import { defaultLineKind } from './oilKind'

describe('defaultLineKind', () => {
  const ml = (container_ml: number | null) => ({ unit: 'ml' as const, container_ml })
  it('тара 150 л — канистра, порог строгий', () => expect(defaultLineKind(ml(150_000), 150)).toBe('container'))
  it('тара 151 л — розлив', () => expect(defaultLineKind(ml(151_000), 150)).toBe('pour'))
  it('порог 0 — всегда канистра', () => expect(defaultLineKind(ml(200_000), 0)).toBe('container'))
  it('объём не задан — канистра', () => expect(defaultLineKind(ml(null), 150)).toBe('container'))
  it('штуки, вес и метры не трогаем', () => {
    expect(defaultLineKind({ unit: 'piece', container_ml: 200_000 }, 150)).toBe('piece')
    expect(defaultLineKind({ unit: 'g', container_ml: null }, 150)).toBe('weight')
    expect(defaultLineKind({ unit: 'mm', container_ml: null }, 150)).toBe('length')
  })
})
