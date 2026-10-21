import { describe, expect, it, vi } from 'vitest'
import { planEnter, resolveEnter, resultsMatch, searchRank } from './pickerResults'

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

describe('Enter в поиске товара (SPEC-30 §13)', () => {
  it('выбирает только из найденного по всему тексту', () => {
    expect(planEnter('масло моторное', 'масло моторное', 3)).toBe('choose')
    expect(planEnter('масло моторное', 'ма', 3)).toBe('search')
    expect(planEnter('масло моторное', '', 0)).toBe('search')
    // По полному тексту ничего — может, это код.
    expect(planEnter('OC90', 'OC90', 0)).toBe('code')
    expect(planEnter('4607001234567', 'ма', 3)).toBe('code')
    expect(planEnter('м', '', 0)).toBe('code')
  })

  it('найдено по началу текста — ждёт ответа на полный текст и берёт первое из него', async () => {
    // 21.10.2026: по «ма» первым шёл воздушный фильтр, а кассир набрал моторное масло.
    const search = vi.fn(async (q: string) => (q === 'масло моторное' ? ['Масло моторное 5W-30', 'Масло моторное 10W-40'] : []))
    const out = await resolveEnter(
      { text: 'масло моторное', foundFor: 'ма', found: ['Воздушный фильтр', 'Масло моторное 5W-30'], active: 0, navigated: false },
      search,
      () => 'масло моторное',
    )
    expect(search).toHaveBeenCalledWith('масло моторное')
    expect(out).toEqual({ kind: 'pick', item: 'Масло моторное 5W-30', fresh: ['Масло моторное 5W-30', 'Масло моторное 10W-40'] })
  })

  it('найдено по всему тексту — выделенная строка, без нового запроса', async () => {
    const search = vi.fn(async () => ['другое'])
    const out = await resolveEnter({ text: 'масло', foundFor: 'масло', found: ['a', 'b'], active: 1, navigated: false }, search, () => 'масло')
    expect(out).toEqual({ kind: 'pick', item: 'b' })
    expect(search).not.toHaveBeenCalled()
  })

  it('строку, выбранную стрелками, кассир выбрал сам', async () => {
    const search = vi.fn(async () => ['первое'])
    const out = await resolveEnter({ text: 'масло 5', foundFor: 'масло', found: ['a', 'b'], active: 1, navigated: true }, search, () => 'масло 5')
    expect(out).toEqual({ kind: 'pick', item: 'b' })
    expect(search).not.toHaveBeenCalled()
  })

  it('по полному тексту ничего — ищем как код; текст поменяли, пока ждали, — ничего не берём', async () => {
    expect(await resolveEnter({ text: 'xyz12', foundFor: 'xy', found: ['a'], active: 0, navigated: false }, async () => [], () => 'xyz12')).toEqual({
      kind: 'code',
      fresh: [],
    })
    expect(await resolveEnter({ text: 'масло', foundFor: 'ма', found: ['a'], active: 0, navigated: false }, async () => ['b'], () => 'масло 5')).toEqual({
      kind: 'stale',
    })
  })

  it('ступени найденного — как на сервере', () => {
    const p = (name: string, article = '', barcodes: string[] = []) => ({ name, article, barcodes })
    expect(searchRank(p('Фильтр', 'OC90'), 'oc90')).toBe(0)
    expect(searchRank(p('Фильтр', '', ['4607001234567']), '4607001234567')).toBe(0)
    expect(searchRank(p('Масло моторное'), 'масло')).toBe(1)
    expect(searchRank(p('Воздушный фильтр для масла'), 'масл')).toBe(2)
  })
})
