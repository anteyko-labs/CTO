import { describe, expect, it } from 'vitest'
import { cartGoods, cartSources, extraChoices, extraSlots, filtersNote, goodShort, goodSlot, nameSlot, nextPayload, nextRows, nextTooLow, oilChangeHint, productSlot, pruneNext, receiptBook, snapshotCategories, type CategoryBook } from './nextChange'
import type { Product, Service } from './types'

const svc = (id: string, name: string, slot: Service['book_slot'], km: number | null, days: number | null = 365): Service => ({
  id,
  name,
  price_tyiyn: 0,
  master_fee_tyiyn: 3000,
  active: true,
  book_slot: slot,
  interval_km: km,
  interval_days: km === null ? null : days,
})

const oil = svc('s1', 'Замена масла в моторе', 'oil', 8000, 31)
const anti = svc('s2', 'Замена антифриза', 'antifreeze', 60000, 730)
const wash = svc('s3', 'Мойка', 'other', null)
const gear = svc('s4', 'Замена масла в коробке', 'gear_oil', 40000, 730)

const cat = (id: string, book_slot: string, km: number | null = null, days: number | null = null): CategoryBook => ({ id, book_slot, book_interval_km: km, book_interval_days: days })
const categories = [
  cat('c-oil', 'oil'),
  // Общая категория фильтров: строка — по типу фильтра товара.
  cat('c-filters', 'oil_filter'),
  cat('c-anti', 'antifreeze', 50000, 700),
  cat('c-plugs', 'none'),
  cat('c-misc', 'other'),
]

const product = (id: string, name: string, category_id: string, attrs: Record<string, string> = {}): Product =>
  ({ id, name, category_id, category_kind: 'other', attrs, unit: 'piece', container_ml: null }) as Product

const motorOil = product('p1', 'Totachi 5W-30 4 л', 'c-oil')
const airFilter = product('p2', 'Фильтр воздушный MANN C-110', 'c-filters', { filter_type: 'воздушный' })
const antifreeze = product('p3', 'Felix G12 5 кг', 'c-anti')
const plug = product('p4', 'Свеча NGK', 'c-plugs')
const misc = product('p5', 'Очиститель', 'c-misc')
const oilFilter = product('p6', 'Фильтр W712', 'c-filters')

/** Чек: масло, воздушный фильтр, антифриз, свеча и услуги замены масла и мойки. */
const cart = [{ product: motorOil }, { product: airFilter }, { product: antifreeze }, { product: plug }, { product: misc }, { service: oil }, { service: wash }]

describe('следующая замена в чеке (SPEC-34)', () => {
  it('по строке на услугу со строкой книжки: пробег сейчас + интервал, у машины свой — он', () => {
    const sources = cartSources([{ service: oil }, { service: wash }, { service: anti }, { service: oil }], categories)
    const rows = nextRows({ sources, extra: [], edits: {}, mileage: '217000', unit: 'km', own: { antifreeze: { km: 33000, days: 700 } } })
    expect(rows.map((r) => [r.label, r.value, r.interval_days])).toEqual([
      ['Замена масла в моторе', '225000', 31],
      ['Замена антифриза', '250000', 700],
    ])
    // Без пробега — поле пустое, пока кассир не впишет.
    expect(nextRows({ sources: cartSources([{ service: oil }], null), extra: [], edits: {}, mileage: '', unit: 'km', own: {} })[0].value).toBe('')
  })

  it('правка кассира и пустое поле — уходят с чеком; меньше пробега — нельзя', () => {
    const sources = cartSources([{ service: oil }, { service: anti }, { service: gear }], categories)
    const rows = nextRows({ sources, extra: [], edits: { oil: '226000', antifreeze: '' }, mileage: '217000', unit: 'km', own: {} })
    expect(nextPayload(rows)).toEqual([
      { service_id: 's1', slot: 'oil', next_mileage: 226000 },
      { service_id: 's2', slot: 'antifreeze', next_mileage: null },
      { service_id: 's4', slot: 'gear_oil', next_mileage: 257000 },
    ])
    expect(nextTooLow(rows, '217000')).toBeNull()
    const low = nextRows({ sources, extra: [], edits: { oil: '216000' }, mileage: '217000', unit: 'km', own: {} })
    expect(nextTooLow(low, '217000')?.slot).toBe('oil')
  })

  it('мили: интервал в км переводится в мили машины', () => {
    const rows = nextRows({ sources: cartSources([{ service: oil }], null), extra: [], edits: {}, mileage: '60000', unit: 'mi', own: {} })
    // 8 000 км ≈ 4 971 миля.
    expect(rows[0].value).toBe('64971')
  })

  it('строка книжки товара — как на сервере: по категории, у фильтров — по типу фильтра', () => {
    expect(productSlot('oil_filter', 'воздушный')).toBe('air_filter')
    expect(productSlot('oil_filter', ' салонный ')).toBe('cabin_filter')
    expect(productSlot('oil_filter', undefined)).toBe('oil_filter')
    expect(productSlot('air_filter', 'непонятный')).toBe('air_filter')
    expect(productSlot('antifreeze', 'воздушный')).toBe('antifreeze')
    expect(productSlot('none', undefined)).toBeNull()
    expect(productSlot('toString', undefined)).toBeNull()
    expect(productSlot(undefined, 'воздушный')).toBeNull()
  })

  it('«+ ещё строку» — только позиции этого чека со строкой книжки, которой ещё нет; справочник услуг — нет', () => {
    const sources = cartSources(cart, categories)
    const rows = nextRows({ sources, extra: [], edits: {}, mileage: '217000', unit: 'km', own: {} })
    // Масло — строкой по услуге; мойка, свеча и «прочее» в книжку не идут.
    expect(rows.map((r) => r.slot)).toEqual(['oil'])
    // Воздушный фильтр при строке масла не предлагается — он на пробег масла (ADR-117).
    expect(extraChoices(sources, rows).map((c) => c.label)).toEqual(['Антифриз (Felix G12 5 кг)'])
    expect(rows[0].filters).toEqual(['воздушный'])
    const noOil = cartSources(
      cart.filter((l) => l.service !== oil),
      categories,
    )
    expect(extraChoices(noOil, []).map((c) => c.label)).toEqual(['Масло двигателя (Totachi 5W-30 4 л)', 'Фильтр воздушный (Фильтр воздушный MANN C-110)', 'Антифриз (Felix G12 5 кг)'])
    // Подарок книжка не пишет — и не предлагается; два фильтра одной строки — одна строка (первый в чеке).
    const gifts = cartSources([{ product: airFilter, gift: true }, { product: oilFilter }, { product: { ...oilFilter, id: 'p7', name: 'Фильтр W713' } }], categories)
    expect(extraChoices(gifts, []).map((c) => c.label)).toEqual(['Фильтр масляный (Фильтр W712)'])
    // Категорий ещё нет — товары не считаются, остаются услуги чека.
    expect(cartSources(cart, null).map((s) => s.slot)).toEqual(['oil'])
    // Чек без товаров со строкой книжки — предложить нечего: кнопки нет.
    const plain = cartSources([{ product: plug }, { service: oil }], categories)
    expect(extraChoices(plain, nextRows({ sources: plain, extra: [], edits: {}, mileage: '1000', unit: 'km', own: {} }))).toEqual([])
  })

  it('строка из товара: пробег + интервал (машины → категории → по умолчанию), уходит строкой книжки без услуги', () => {
    // Чек без масла двигателя (замена масла в коробке): воздушный фильтр — своей строкой.
    const gearCart = cart.map((l) => (l.service === oil ? { service: gear } : l))
    const sources = cartSources(gearCart, categories)
    const rows = nextRows({ sources, extra: ['air_filter', 'antifreeze'], edits: {}, mileage: '217000', unit: 'km', own: {} })
    expect(rows.map((r) => [r.label, r.value, r.interval_days, r.extra])).toEqual([
      ['Замена масла в коробке', '257000', 730, false],
      // Воздушный фильтр: у категории интервала нет — по умолчанию строки, 15 000 км или год.
      ['Фильтр воздушный (Фильтр воздушный MANN C-110)', '232000', 365, true],
      // Антифриз: интервал категории.
      ['Антифриз (Felix G12 5 кг)', '267000', 700, true],
    ])
    expect(nextPayload(rows)).toEqual([
      { service_id: 's4', slot: 'gear_oil', next_mileage: 257000 },
      { slot: 'air_filter', next_mileage: 232000 },
      { slot: 'antifreeze', next_mileage: 267000 },
    ])
    expect(extraChoices(sources, rows).map((c) => c.slot)).toEqual(['oil'])
    // У машины свой интервал воздушного фильтра; машина в милях — в милях.
    const own = nextRows({ sources, extra: ['air_filter'], edits: {}, mileage: '217000', unit: 'km', own: { air_filter: { km: 10000, days: 180 } } })
    expect([own[1].value, own[1].interval_days]).toEqual(['227000', 180])
    expect(nextRows({ sources, extra: ['air_filter'], edits: {}, mileage: '60000', unit: 'mi', own: {} })[1].value).toBe('69321')
    // Добавили услугу той же строки — строка встаёт по услуге, «+ ещё» по товару уже не нужна.
    const withService = cartSources([...gearCart, { service: svc('s9', 'Замена воздушного фильтра', 'air_filter', 15000) }], categories)
    expect(nextRows({ sources: withService, extra: ['air_filter'], edits: {}, mileage: '217000', unit: 'km', own: {} }).map((r) => [r.label, r.extra])).toEqual([
      ['Замена масла в коробке', false],
      ['Замена воздушного фильтра', false],
    ])
  })

  it('фильтры с заменой масла двигателя — без своих строк, на пробег масла (ADR-117)', () => {
    const cabin = product('p8', 'Фильтр салонный CU-2', 'c-filters', { filter_type: 'салонный' })
    const fuel = product('p9', 'Фильтр топливный WK-842', 'c-filters', { filter_type: 'топливный' })
    const sources = cartSources(
      [{ product: oilFilter }, { product: airFilter }, { product: cabin }, { product: fuel }, { service: svc('s9', 'Замена воздушного фильтра', 'air_filter', 15000) }, { service: oil }],
      categories,
    )
    const rows = nextRows({ sources, extra: ['oil_filter', 'fuel_filter'], edits: { air_filter: '230000' }, mileage: '250000', unit: 'km', own: {} })
    // Услуга воздушного фильтра и «+ ещё» масляного своих строк не дают; топливный — своей строкой.
    expect(rows.map((r) => [r.slot, r.value])).toEqual([
      ['oil', '258000'],
      ['fuel_filter', '280000'],
    ])
    expect(filtersNote(rows[0])).toBe('и фильтры: масляный, воздушный, салонный — на тот же пробег')
    expect(nextPayload(rows)).toEqual([
      { service_id: 's1', slot: 'oil', next_mileage: 258000 },
      { slot: 'fuel_filter', next_mileage: 280000 },
    ])
    expect(extraChoices(sources, rows)).toEqual([])
    expect(filtersNote({ filters: ['воздушный'] })).toBe('и фильтр: воздушный — на тот же пробег')
    expect(filtersNote({})).toBe('')
  })

  it('«Это замена?» — канистра моторного масла без услуги масла двигателя (ADR-117)', () => {
    const enough = new Set(['c-oil'])
    expect(oilChangeHint([{ product: motorOil }, { service: wash }], categories, enough)).toBe(true)
    expect(oilChangeHint([{ product: motorOil }, { service: oil }], categories, enough)).toBe(false)
    // Меньше канистры — доливка, подсказки нет; масло коробки — не моторное.
    expect(oilChangeHint([{ product: motorOil }], categories, new Set())).toBe(false)
    expect(oilChangeHint([{ product: motorOil }], [cat('c-oil', 'gear_oil')], enough)).toBe(false)
  })

  it('интервал категории — только строки этой категории, как в книжке: общая «Фильтры» не задаёт срок воздушному', () => {
    // Владелец задал общей категории фильтров (строка — масляный фильтр) 9 000 км / 200 дней.
    const shared = [cat('c-oil', 'oil'), cat('c-filters', 'oil_filter', 9000, 200)]
    const lines = [{ product: airFilter }, { product: oilFilter }]
    const rows = (cats: CategoryBook[]) =>
      nextRows({ sources: cartSources(lines, cats), extra: ['air_filter', 'oil_filter'], edits: {}, mileage: '217000', unit: 'km', own: {} }).map((r) => [r.slot, r.value, r.interval_days])
    expect(rows(shared)).toEqual([
      // Воздушный фильтр — по умолчанию строки (15 000 км или год), а не интервал масляного.
      ['air_filter', '232000', 365],
      // Масляный фильтр той же категории — её интервал.
      ['oil_filter', '226000', 200],
    ])
    // Есть категория воздушных фильтров со своим интервалом — у воздушного фильтра из общей категории
    // он (как `slot_next` на сервере: любая категория этой строки).
    expect(rows([...shared, cat('c-air', 'air_filter', 20000, 400)])).toEqual([
      ['air_filter', '237000', 400],
      ['oil_filter', '226000', 200],
    ])
    // У своей категории товара интервала нет, у другой категории этой строки — есть: он.
    expect(rows([cat('c-filters', 'oil_filter'), cat('c-oil-filters', 'oil_filter', 10000, 180)])).toEqual([
      ['air_filter', '232000', 365],
      ['oil_filter', '227000', 180],
    ])
  })

  it('удалили товар из чека — строка «+ ещё» по нему и правка кассира уходят', () => {
    const before = cartSources(cart, categories)
    expect(pruneNext(['air_filter'], { oil: '226000', air_filter: '233000' }, before)).toBeNull()
    const after = cartSources(
      cart.filter((l) => l.product !== airFilter),
      categories,
    )
    expect(pruneNext(['air_filter'], { oil: '226000', air_filter: '233000' }, after)).toEqual({ extra: [], edits: { oil: '226000' } })
    expect(nextRows({ sources: after, extra: ['air_filter'], edits: {}, mileage: '217000', unit: 'km', own: {} }).map((r) => r.slot)).toEqual(['oil'])
    // Удалили услугу — правка по её строке тоже уходит.
    const noService = cartSources([{ product: airFilter }], categories)
    expect(pruneNext(['air_filter'], { oil: '226000', air_filter: '' }, noService)).toEqual({ extra: ['air_filter'], edits: { air_filter: '' } })
  })

  it('черновик и отложенный чек: строки «+ ещё» — строки книжки; прежние услуги справочника не берутся', () => {
    expect(extraSlots(['air_filter', 'antifreeze', 'brakes', 7])).toEqual(['air_filter', 'antifreeze'])
    expect(extraSlots([anti])).toEqual([])
    expect(extraSlots(undefined)).toEqual([])
  })

  it('снимок кассы: категории со строкой книжки; старый снимок — как не загружены', () => {
    expect(snapshotCategories([{ id: 'c1', book_slot: 'air_filter', book_interval_km: 15000, book_interval_days: 365 }, { id: 'c2', book_slot: 'none' }])).toEqual([
      cat('c1', 'air_filter', 15000, 365),
      cat('c2', 'none'),
    ])
    expect(snapshotCategories([{ id: 'c1' }])).toBeNull()
    expect(snapshotCategories(undefined)).toBeNull()
  })

  it('книжка для чека из очереди — как отдаст сервер: только жидкости, что залито и непустые строки (ADR-116)', () => {
    const lines = [
      { service: oil },
      { service: anti },
      { product: airFilter, kind: 'piece', qtyText: '1' },
      { product: antifreeze, kind: 'piece', qtyText: '2' },
      { product: { ...motorOil, id: 'p10', container_ml: 4000 }, kind: 'container', qtyText: '1' },
      { product: { ...motorOil, id: 'p11', name: 'Totachi 5W-30 розлив' }, kind: 'pour', qtyText: '0,5' },
    ]
    const sources = cartSources(lines, categories)
    const rows = nextRows({ sources, extra: ['air_filter'], edits: { antifreeze: '' }, mileage: '217000', unit: 'km', own: {} })
    const b = receiptBook({ brand: 'Toyota', model: 'Camry', plate: '01KG123ABC' }, '217000', 'km', rows, '2026-10-26', cartGoods(lines, categories))
    expect(b.lines).toEqual([{ label: 'Масло двигателя', slot: 'oil', next_mileage: 225000, unit: 'km', next_date: '2026-11-26' }])
    expect(b.filled).toEqual([
      { slot: 'oil', label: 'Масло двигателя', product: 'Totachi 5W-30 4 л · 1 кан. × 4 л' },
      { slot: 'oil', label: 'Масло двигателя', product: 'Totachi 5W-30 розлив · 0,5 л' },
      { slot: 'antifreeze', label: 'Антифриз', product: 'Felix G12 5 кг · 2 шт' },
    ])
    expect(b.mileage).toBe(217000)
    // Масло клиента — услуга без товара.
    const own = nextRows({ sources: cartSources([{ service: oil }], categories), extra: [], edits: {}, mileage: '217000', unit: 'km', own: {} })
    expect(receiptBook({ brand: '', model: '', plate: '1' }, '217000', 'km', own, '2026-10-26').filled).toEqual([{ slot: 'oil', label: 'Масло двигателя', product: 'масло клиента' }])
    // На кассе под строкой — товар коротко.
    expect(cartGoods(lines, categories).map(goodShort)).toEqual([
      'Фильтр воздушный MANN C-110 × 1',
      'Felix G12 5 кг × 2',
      'Totachi 5W-30 4 л × 1 кан.',
      'Totachi 5W-30 розлив × 0,5 л',
    ])
  })

  it('товар категории без строки книжки — жидкость по названию, как на сервере (ADR-130)', () => {
    expect(nameSlot('Антифриз Sintec G12 красный 5 кг')).toBe('antifreeze')
    expect(nameSlot('ТОСОЛ А-40М')).toBe('antifreeze')
    expect(nameSlot('Охлаждающая жидкость Felix')).toBe('antifreeze')
    expect(nameSlot('Тормозная жидкость DOT-4')).toBe('brake_fluid')
    expect(nameSlot('Свеча NGK')).toBeNull()
    // Строка категории важнее названия; «прочее» — не жидкость.
    expect(goodSlot(cat('c-misc', 'other'), { name: 'Антифриз G12', attrs: {} })).toBe('other')
    expect(goodSlot(undefined, { name: 'Антифриз G12', attrs: {} })).toBeNull()
    // Рабочая база: «Антифриз и жидкости» без строки книжки + услуга «Замена антифриза».
    const sintec = product('p20', 'Антифриз Sintec G12 красный 5 кг', 'c-plugs')
    const lines = [{ product: sintec, kind: 'piece', qtyText: '1' }, { service: anti }]
    const goods = cartGoods(lines, categories)
    expect(goods.map((g) => g.slot)).toEqual(['antifreeze'])
    const rows = nextRows({ sources: cartSources(lines, categories), extra: [], edits: {}, mileage: '120000', unit: 'km', own: {} })
    const b = receiptBook({ brand: '', model: '', plate: '1' }, '120000', 'km', rows, '2026-10-27', goods)
    expect(b.filled).toEqual([{ slot: 'antifreeze', label: 'Антифриз', product: 'Антифриз Sintec G12 красный 5 кг · 1 шт' }])
  })
})
