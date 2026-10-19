import { describe, expect, it } from 'vitest'
import { resultsMatch } from './pickerResults'

describe('найденное в поиске товара', () => {
  it('тот же запрос, набранный снова после выбора, показывает найденное', () => {
    expect(resultsMatch('масло', 'масло')).toBe(true)
    expect(resultsMatch('  масло ', 'масло')).toBe(true)
  })

  it('пока набор продолжает запрос — показываем прежнее', () => {
    expect(resultsMatch('масло 5w', 'масло')).toBe(true)
  })

  it('пустое поле или другой текст — не показываем', () => {
    expect(resultsMatch('', 'масло')).toBe(false)
    expect(resultsMatch('фильтр', 'масло')).toBe(false)
    expect(resultsMatch('мас', 'масло')).toBe(false)
  })
})
