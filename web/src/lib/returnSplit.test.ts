import { describe, expect, it } from 'vitest'
import { returnDebtCap, splitReturn } from './returnSplit'

describe('деление возврата долгового чека (SPEC-36 §8)', () => {
  it('с долга — не больше долга по чеку и долга клиента сейчас', () => {
    expect(returnDebtCap(40_000, 90_000)).toBe(40_000)
    expect(returnDebtCap(100_000, 30_000)).toBe(30_000)
    // Аванс клиента и полностью снятый долг — с долга нечего.
    expect(returnDebtCap(100_000, -5_000)).toBe(0)
    expect(returnDebtCap(0, 50_000)).toBe(0)
  })

  it('по умолчанию сначала долг, остаток деньгами', () => {
    // Чек «600 наличными + 400 в долг» возвращается целиком: 400 с долга, 600 деньгами.
    expect(splitReturn(100_000, 40_000, null)).toEqual({ debt: 40_000, money: 60_000, ok: true })
    // Возврат меньше долга — весь с долга.
    expect(splitReturn(25_000, 40_000, null)).toEqual({ debt: 25_000, money: 0, ok: true })
    expect(splitReturn(100_000, 0, null)).toEqual({ debt: 0, money: 100_000, ok: true })
  })

  it('правка одной суммы пересчитывает другую', () => {
    expect(splitReturn(100_000, 30_000, { field: 'debt', value: 10_000 })).toEqual({ debt: 10_000, money: 90_000, ok: true })
    expect(splitReturn(100_000, 30_000, { field: 'money', value: 80_000 })).toEqual({ debt: 20_000, money: 80_000, ok: true })
  })

  it('с долга больше допустимого, минус и не число — ошибка', () => {
    expect(splitReturn(100_000, 30_000, { field: 'debt', value: 50_000 }).ok).toBe(false)
    expect(splitReturn(100_000, 30_000, { field: 'money', value: 60_000 }).ok).toBe(false)
    expect(splitReturn(100_000, 30_000, { field: 'money', value: 120_000 }).ok).toBe(false)
    expect(splitReturn(100_000, 30_000, { field: 'debt', value: null }).ok).toBe(false)
  })
})
