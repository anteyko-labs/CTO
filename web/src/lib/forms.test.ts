import { describe, expect, it } from 'vitest'
import { missing, missingWithFocus } from './forms'

describe('незаполненное', () => {
  it('перечисляет только незаполненные поля в порядке экрана', () => {
    expect(
      missing([true, 'кассира'], [false, 'мастера'], [false, 'оплату'], [true, 'товары']),
    ).toEqual(['мастера', 'оплату'])
  })

  it('пустой список, когда всё заполнено', () => {
    expect(missing([true, 'кассира'], [true, 'товары'])).toEqual([])
  })

  it('сохраняет поле для перехода фокуса', () => {
    expect(missingWithFocus([false, 'кассира', '#cashier-select'], [false, 'оплату'])).toEqual([
      { label: 'кассира', focus: '#cashier-select' },
      { label: 'оплату', focus: undefined },
    ])
  })
})
