import { describe, expect, it } from 'vitest'
import { addCodes, normCode, pickProgress, splitCodes } from './crossPick'

describe('кросс-номера карточки (задача 002 П10)', () => {
  it('номер сравнивается без пробелов, дефисов и косых', () => {
    expect(normCode('W 712/75')).toBe('W71275')
    expect(normCode('90915-yzze1')).toBe('90915YZZE1')
  })

  it('поле делится по запятой, точке с запятой и строке', () => {
    expect(splitCodes('OC 217; W712/75\n 90915-YZZE1, ')).toEqual(['OC 217', 'W712/75', '90915-YZZE1'])
  })

  it('подобранные номера дописываются без повторов', () => {
    expect(addCodes('W 712/75', ['W712/75', '90915-YZZE1', ' OC 217 ', '90915YZZE1'])).toBe('W 712/75, 90915-YZZE1, OC 217')
    expect(addCodes('', ['LF-1'])).toBe('LF-1')
  })

  it('подбор по категории показывает, сколько подборов осталось', () => {
    expect(pickProgress(3, 12, { used_today: 3, daily_limit: 50 })).toBe('Обработано 3 из 12, подборов на сегодня осталось 47')
    expect(pickProgress(0, 5, null)).toBe('Обработано 0 из 5')
  })
})
