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

  it('найдено по началу текста — ждёт ответа на полный текст и берёт найденное по нему', async () => {
    // 21.10.2026: по «ма» первым шёл воздушный фильтр, а кассир набрал моторное масло.
    const search = vi.fn(async (q: string) => (q === 'масло моторное' ? ['Масло моторное 5W-30'] : []))
    const out = await resolveEnter(
      { text: 'масло моторное', foundFor: 'ма', found: ['Воздушный фильтр', 'Масло моторное 5W-30'], active: 0, navigated: false },
      search,
      () => 'масло моторное',
    )
    expect(search).toHaveBeenCalledWith('масло моторное')
    expect(out).toEqual({ kind: 'pick', item: 'Масло моторное 5W-30', fresh: ['Масло моторное 5W-30'] })
  })

  it('найдено по всему тексту — без нового запроса: одна строка — она, несколько — выбирает кассир', async () => {
    const search = vi.fn(async () => ['другое'])
    expect(await resolveEnter({ text: 'масло', foundFor: 'масло', found: ['a'], active: 0, navigated: false }, search, () => 'масло')).toEqual({ kind: 'pick', item: 'a' })
    // Строка под мышью (active) без стрелок — ещё не выбор.
    expect(await resolveEnter({ text: 'масло', foundFor: 'масло', found: ['a', 'b'], active: 1, navigated: false }, search, () => 'масло')).toEqual({ kind: 'ambiguous' })
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

describe('Enter берёт товар, только если выбор однозначен (исправлено 26.10.2026)', () => {
  // «В чеке постоянно добавляет воздушный фильтр»: Enter клал первую из нескольких найденных строк,
  // а первым по названию почти всегда шёл «Фильтр воздушный …».
  type Item = { name: string; article: string; barcodes: string[] }
  const item = (name: string, barcodes: string[] = [], article = ''): Item => ({ name, article, barcodes })
  const exact = (x: Item, q: string) => searchRank(x, q) === 0
  const air = item('Фильтр воздушный Ларгус', ['4600000000011'])
  const oilF = item('Фильтр масляный Ларгус', ['4600000000028'])
  const cabin = item('Фильтр салонный Ларгус', ['4600000000035'])

  it('найдено несколько по всему тексту — Enter ничего не кладёт, кассир выбирает строку', async () => {
    const search = vi.fn(async () => [air])
    const out = await resolveEnter({ text: 'ларгус', foundFor: 'ларгус', found: [air, oilF, cabin], active: 0, navigated: false }, search, () => 'ларгус', exact)
    expect(out).toEqual({ kind: 'ambiguous' })
    expect(search).not.toHaveBeenCalled()
  })

  it('Enter сразу после набора: в ответе несколько — тоже выбрать строку', async () => {
    const out = await resolveEnter({ text: 'ларгус', foundFor: '', found: [], active: 0, navigated: false }, async () => [air, oilF], () => 'ларгус', exact)
    expect(out).toEqual({ kind: 'ambiguous', fresh: [air, oilF] })
  })

  it('однозначно — берёт: одна строка, точный код или артикул среди нескольких, строка выбрана стрелками', async () => {
    expect(await resolveEnter({ text: 'салонный', foundFor: 'салонный', found: [cabin], active: 0, navigated: false }, async () => [], () => 'салонный', exact)).toEqual({
      kind: 'pick',
      item: cabin,
    })
    const art = item('Фильтр масляный W712', [], 'W712')
    expect(
      await resolveEnter({ text: 'w712', foundFor: '', found: [], active: 0, navigated: false }, async () => [item('Фильтр W712/75'), art], () => 'w712', exact),
    ).toEqual({ kind: 'pick', item: art, fresh: [item('Фильтр W712/75'), art] })
    expect(await resolveEnter({ text: 'ларгус', foundFor: 'ларгус', found: [air, oilF, cabin], active: 1, navigated: true }, async () => [], () => 'ларгус', exact)).toEqual({
      kind: 'pick',
      item: oilF,
    })
  })

  it('короткий код из цифр (доп. код Умага «1024») — только точный код, не товар с этими цифрами в названии', async () => {
    // Масло «АГРИНОЛ 10W40 200Л» с кодом 1024 в Умаге; по тексту «1024» первым шёл «E1024L01 ВОЗДУШКА РОБОТ».
    const filter = item('E1024L01 ВОЗДУШКА РОБОТ', ['2110000045357'])
    const mft = item('MFT-1024 (SG1068)')
    expect(await resolveEnter({ text: '1024', foundFor: '1024', found: [filter, mft], active: 0, navigated: false }, async () => [], () => '1024', exact)).toEqual({ kind: 'code' })
    expect(await resolveEnter({ text: '1024', foundFor: '', found: [], active: 0, navigated: false }, async () => [filter], () => '1024', exact)).toEqual({
      kind: 'code',
      fresh: [filter],
    })
    const agrinol = item('АГРИНОЛ 10W40 200Л', ['1024'])
    expect(await resolveEnter({ text: '1024', foundFor: '1024', found: [filter, agrinol], active: 0, navigated: false }, async () => [], () => '1024', exact)).toEqual({
      kind: 'pick',
      item: agrinol,
    })
  })
})
