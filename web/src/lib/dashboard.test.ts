import { describe, expect, it } from 'vitest'
import { earnedList, minusSom, payReason, signedSom, vsYesterday, weekAgo } from './dashboard'
import { formatSom } from './format'
import type { StaffPay } from './types'

const nb = (s: string | null | undefined) => (s ?? '').replace(/ /g, ' ')
const som = (v: number) => formatSom(v)

const person = (name: string, items: StaffPay['items']): StaffPay => ({
  employee_id: name,
  name,
  items,
  total_tyiyn: items.reduce((a, i) => a + i.amount_tyiyn, 0),
})

describe('сводка владельца', () => {
  it('за что начислено — коротко, удержание со знаком минус', () => {
    expect(
      nb(
        payReason([
          { kind: 'revenue_percent', count: 3, amount_tyiyn: 800 },
          { kind: 'service_fee', count: 2, amount_tyiyn: 6000 },
        ]),
      ),
    ).toBe('% с чеков 8 с · замены ×2 60 с')
    expect(nb(payReason([{ kind: 'shortage', count: -1, amount_tyiyn: -5000 }]))).toBe('удержание недостачи −50 с')
    expect(nb(payReason([{ kind: 'monthly_salary', count: 1, amount_tyiyn: 3_000_000 }]))).toBe('оклад 30 000 с')
  })

  it('список: только с начислениями, больше — выше', () => {
    const list = earnedList([
      person('Борис', [{ kind: 'shortage', count: -1, amount_tyiyn: -5000 }]),
      person('Пустой', []),
      person('Анна', [{ kind: 'service_fee', count: 1, amount_tyiyn: 3000 }]),
      person('Вера', [{ kind: 'revenue_percent', count: 5, amount_tyiyn: 9000 }]),
    ])
    expect(list.map((p) => p.name)).toEqual(['Вера', 'Анна', 'Борис'])
  })

  it('сравнение с вчера: коротко, без переноса', () => {
    expect(nb(vsYesterday(14000, 10000, som)?.text)).toBe('▲ 40 % к вчера')
    expect(vsYesterday(14000, 10000, som)?.tone).toBe('up')
    expect(nb(vsYesterday(9000, 10000, som)?.text)).toBe('▼ 10 % к вчера')
    expect(vsYesterday(10000, 10000, som)?.tone).toBe('flat')
    expect(nb(vsYesterday(100000, 1000, som)?.text)).toBe('▲ вчера: 10 с')
    expect(vsYesterday(5, 0, String)).toBeNull()
    expect(vsYesterday(7, 5, String)?.text).toBe('▲ 40 % к вчера')
  })

  it('неделю назад — сумма тогда', () => {
    expect(nb(weekAgo(920000, som))).toBe('нед. назад: 9 200 с')
    expect(weekAgo(12, String)).toBe('нед. назад: 12')
    expect(weekAgo(0, som)).toBeNull()
  })

  it('знак минус прямо перед числом', () => {
    expect(nb(minusSom(8540000))).toBe('−85 400 с')
    expect(nb(minusSom(0))).toBe('0 с')
    expect(nb(signedSom(-30000))).toBe('−300 с')
    expect(nb(signedSom(30000))).toBe('+300 с')
    expect(nb(signedSom(0))).toBe('0 с')
  })
})
