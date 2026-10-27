import { describe, expect, it } from 'vitest'
import { accountNumberProblem, countdown, periodDays, qrForCard, qrPaidSum, secondsLeft, strayQr, type BankQr } from './optima'

describe('QR банка на кассе (SPEC-31)', () => {
  it('отсчёт от ответа сервера', () => {
    expect(secondsLeft({ seconds_left: 600, received: 1_000 }, 1_000)).toBe(600)
    expect(secondsLeft({ seconds_left: 600, received: 1_000 }, 63_500)).toBe(538)
    expect(secondsLeft({ seconds_left: 5, received: 0 }, 60_000)).toBe(0)
    expect(countdown(600)).toBe('10:00')
    expect(countdown(65)).toBe('1:05')
    expect(countdown(-3)).toBe('0:00')
  })

  it('оплата картой ссылается только на оплаченный QR той же суммы', () => {
    const qr = { id: 'q', status: 'paid' as const, amount_tyiyn: 99_000 }
    expect(qrForCard(qr, 99_000)).toBe('q')
    expect(qrForCard(qr, 98_000)).toBeUndefined()
    expect(qrForCard({ ...qr, status: 'waiting' }, 99_000)).toBeUndefined()
    expect(qrForCard(null, 99_000)).toBeUndefined()
  })

  it('сумма оплаты по банку должна совпасть с оплатой картой (ADR-130)', () => {
    const qr = { id: 'q', status: 'paid' as const, amount_tyiyn: 99_000, paid_tyiyn: 98_000 }
    expect(qrPaidSum(qr)).toBe(98_000)
    expect(qrForCard(qr, 99_000)).toBeUndefined()
    expect(qrForCard({ ...qr, paid_tyiyn: 99_000 }, 99_000)).toBe('q')
    expect(qrPaidSum({ amount_tyiyn: 5_000, paid_tyiyn: null })).toBe(5_000)
  })

  it('QR кассы не на экране: сначала оплаченный без чека, ждущий — только когда окна QR нет', () => {
    const base: BankQr = { id: 'a', amount_tyiyn: 1_000, status: 'waiting', expires_at: '', seconds_left: 60, paid_at: null, sale_id: null }
    const waiting = base
    const paid: BankQr = { ...base, id: 'b', status: 'paid', paid_tyiyn: 1_000 }
    expect(strayQr([], undefined)).toBeNull()
    expect(strayQr([waiting, paid], undefined)?.id).toBe('b')
    expect(strayQr([waiting], undefined)?.id).toBe('a')
    expect(strayQr([waiting], 'x')).toBeNull()
    expect(strayQr([paid], 'b')).toBeNull()
    expect(strayQr([{ ...paid, sale_id: 's' }], undefined)).toBeNull()
  })

  it('настройка и период сверки', () => {
    expect(accountNumberProblem('1091 8201 0004 0101')).toBeNull()
    expect(accountNumberProblem('')).toBeNull()
    expect(accountNumberProblem('12345')).toBe('расчётный счёт — 16 цифр')
    expect(periodDays('2026-10-01', '2026-10-14')).toBe(14)
    expect(periodDays('2026-10-21', '2026-10-21')).toBe(1)
  })
})
