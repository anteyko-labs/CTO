import { describe, expect, it } from 'vitest'
import { payeeHints, payeeMissing, payrollLikeArticle } from './expensePayee'

describe('«Кому» у расхода', () => {
  it('обязательное поле по настройке', () => {
    expect(payeeMissing('required', '  ')).toBe(true)
    expect(payeeMissing('required', 'Иман')).toBe(false)
    expect(payeeMissing('optional', '')).toBe(false)
    expect(payeeMissing('off', '')).toBe(false)
    expect(payeeMissing(undefined, '')).toBe(false)
  })

  it('подсказки без повторов и пустых', () => {
    expect(payeeHints(['Иман', 'иман', ' ', 'Электросети', 'Алексей'])).toEqual(['Иман', 'Электросети', 'Алексей'])
    expect(payeeHints(['а', 'б', 'в'], 2)).toEqual(['а', 'б'])
  })

  it('статья, похожая на зарплату, — предупреждение о двойном учёте', () => {
    for (const n of ['ЗАРПЛАТА ИМАН', 'Зарплата', 'Оклад Алексею', 'Аванс', 'ДОП ДОХОД РОМА', 'Доп. доход']) expect(payrollLikeArticle(n), n).toBe(true)
    for (const n of ['Хозтовары', 'Коммунальные расходы', 'расход семья', 'Закуп мелочей']) expect(payrollLikeArticle(n), n).toBe(false)
  })
})
