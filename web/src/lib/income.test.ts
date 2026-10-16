import { describe, expect, it } from 'vitest'
import { normalizeRange, periodDays, quickOf, quickRange, rangeTitle, sortDebts } from './income'
import { VS_PREV_DAY, VS_PREV_PERIOD, vsYesterday } from './dashboard'
import type { DebtRow } from './types'

const nb = (s: string | null | undefined) => (s ?? '').replace(/ /g, ' ')

const row = (name: string, extra: Partial<DebtRow>): DebtRow => ({
  id: name,
  role: 'customer',
  kind: 'person',
  name,
  phone: '',
  inn: '',
  comment: '',
  credit_limit_tyiyn: null,
  due_days: null,
  active: true,
  balance_tyiyn: 0,
  overdue_tyiyn: 0,
  due_date: null,
  overdue_days: 0,
  last_payment_at: null,
  ...extra,
})

describe('период сводки', () => {
  const today = '2026-10-16'

  it('быстрые кнопки и свой выбор', () => {
    expect(quickRange('week', today)).toEqual({ from: '2026-10-10', to: today })
    expect(quickOf({ from: '2026-09-17', to: today }, today)).toBe('month')
    expect(quickOf({ from: '2026-10-15', to: '2026-10-15' }, today)).toBeNull()
    expect(rangeTitle({ from: today, to: today }, today)).toBe('сегодня')
    expect(rangeTitle({ from: '2026-10-15', to: '2026-10-15' }, today)).toBe('за 15.10.2026')
    expect(rangeTitle({ from: '2026-10-01', to: '2026-10-12' }, today)).toBe('с 01.10.2026 по 12.10.2026')
  })

  it('начало позже конца — меняем местами; длиннее года — обрезаем начало', () => {
    expect(normalizeRange('2026-10-12', '2026-10-01')).toEqual({ from: '2026-10-01', to: '2026-10-12' })
    const r = normalizeRange('2020-01-01', today)
    expect(periodDays(r.from, r.to)).toBe(367)
    expect(r.to).toBe(today)
    expect(periodDays('2026-10-16', '2026-10-16')).toBe(1)
  })

  it('сравнение прошедшего дня и периода — своими словами', () => {
    expect(nb(vsYesterday(150, 100, String, VS_PREV_DAY)?.text)).toBe('▲ 50 % к пред. дню')
    expect(nb(vsYesterday(50, 100, String, VS_PREV_PERIOD)?.text)).toBe('▼ 50 % к пред. периоду')
    expect(vsYesterday(50, 0, String, VS_PREV_PERIOD)).toBeNull()
  })
})

describe('экран «Долги»', () => {
  const rows = [
    row('Бакыт', { balance_tyiyn: 5_000, phone: '0555', overdue_days: 3, credit_limit_tyiyn: 10_000, last_payment_at: '2026-10-01T10:00:00Z' }),
    row('Айбек', { balance_tyiyn: 20_000, overdue_days: 0 }),
    row('Вика', { balance_tyiyn: -7_000, phone: '0700', last_payment_at: '2026-10-10T10:00:00Z' }),
  ]

  it('по долгу — по модулю, крупные сверху', () => {
    expect(sortDebts(rows, 'debt', true).map((r) => r.name)).toEqual(['Айбек', 'Вика', 'Бакыт'])
  })

  it('пустые значения всегда внизу', () => {
    expect(sortDebts(rows, 'phone', false).map((r) => r.name)).toEqual(['Бакыт', 'Вика', 'Айбек'])
    expect(sortDebts(rows, 'phone', true).map((r) => r.name)).toEqual(['Вика', 'Бакыт', 'Айбек'])
    expect(sortDebts(rows, 'limit', true).map((r) => r.name)).toEqual(['Бакыт', 'Айбек', 'Вика'])
    expect(sortDebts(rows, 'last_payment', true).map((r) => r.name)).toEqual(['Вика', 'Бакыт', 'Айбек'])
  })

  it('по просрочке и по имени', () => {
    expect(sortDebts(rows, 'overdue', true)[0].name).toBe('Бакыт')
    expect(sortDebts(rows, 'name', false).map((r) => r.name)).toEqual(['Айбек', 'Бакыт', 'Вика'])
  })
})
