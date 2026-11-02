import { describe, expect, it } from 'vitest'
import {
  cartGoods,
  cartSources,
  extraChoices,
  extraServices,
  filtersNote,
  goodBook,
  goodShort,
  goodSlot,
  nameSlot,
  nextEditsOf,
  nextFromDraft,
  nextPayload,
  nextRows,
  nextTooFar,
  nextTooLow,
  oilChangeHint,
  productSlot,
  pruneNext,
  receiptBook,
  serviceKey,
  snapshotCategories,
  soldSources,
  type CategoryBook,
  type NextEdits,
} from './nextChange'
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
// Пример заказчика 27.10.2026 (ADR-131): замена со своим маслом, ГУР и кардан — «прочее».
const ownOil = svc('s5', 'Замена со своим маслом', 'oil', 8000, 31)
const gur = svc('s6', 'Замена ГУР', 'other', null)
const shaft = svc('s7', 'Смазка кардана, 1 точка', 'other', 20000, 180)
const topup = { ...svc('s8', 'Доливка масла', 'oil', 8000, 31), takeaway: true }

const cat = (id: string, book_slot: string, km: number | null = null, days: number | null = null): CategoryBook => ({ id, book_slot, book_interval_km: km, book_interval_days: days })
const categories = [
  cat('c-oil', 'oil'),
  // Общая категория фильтров: строка — по типу фильтра товара.
  cat('c-filters', 'oil_filter'),
  cat('c-anti', 'antifreeze', 50000, 700),
  cat('c-gear', 'gear_oil'),
  cat('c-plugs', 'none'),
  cat('c-misc', 'other'),
]

const product = (id: string, name: string, category_id: string, attrs: Record<string, string> = {}): Product =>
  ({ id, name, category_id, category_kind: 'other', attrs, unit: 'piece', container_ml: null }) as Product

const motorOil = product('p1', 'Totachi 5W-30 4 л', 'c-oil')
const airFilter = product('p2', 'Фильтр воздушный MANN C-110', 'c-filters', { filter_type: 'воздушный' })
const antifreeze = product('p3', 'Felix G12 5 кг', 'c-anti')
const plug = product('p4', 'Свеча NGK', 'c-plugs')
const oilFilter = product('p6', 'Фильтр W712', 'c-filters')
const atf = product('p7', 'ATF Dexron III', 'c-gear')

/** Чек заказчика: три услуги, ATF 1 л, антифриз и воздушный фильтр. */
const sample = [{ service: ownOil }, { service: gur }, { service: shaft }, { product: atf }, { product: antifreeze }, { product: airFilter }]

const rowsOf = (lines: Parameters<typeof cartSources>[0], extra: string[] = [], edits: NextEdits = {}, mileage = '250000') =>
  nextRows({ sources: cartSources(lines), goods: cartGoods(lines, categories), extra, edits, mileage, unit: 'km', own: {} })

describe('следующая замена в чеке (SPEC-34)', () => {
  it('по строке на услугу со строкой книжки: пробег сейчас + интервал, у машины свой — он', () => {
    const sources = cartSources([{ service: oil }, { service: wash }, { service: anti }, { service: oil }, { service: topup }])
    const rows = nextRows({ sources, extra: [], edits: {}, mileage: '217000', unit: 'km', own: { antifreeze: { km: 33000, days: 700 } } })
    // «Прочее» само строкой не встаёт; доливка на вынос — не замена (ADR-118).
    expect(rows.map((r) => [r.key, r.label, r.value, r.interval_days])).toEqual([
      ['oil', 'Замена масла в моторе', '225000', 31],
      ['antifreeze', 'Замена антифриза', '250000', 700],
    ])
    // Без пробега — поле пустое, пока кассир не впишет.
    expect(nextRows({ sources: cartSources([{ service: oil }]), extra: [], edits: {}, mileage: '', unit: 'km', own: {} })[0].value).toBe('')
  })

  it('правка кассира и пустое поле — уходят с чеком; меньше пробега — нельзя', () => {
    const sources = cartSources([{ service: oil }, { service: anti }, { service: gear }])
    const rows = nextRows({ sources, extra: [], edits: { oil: '226000', antifreeze: '' }, mileage: '217000', unit: 'km', own: {} })
    expect(nextPayload(rows)).toEqual([
      { service_id: 's1', slot: 'oil', next_mileage: 226000 },
      { service_id: 's2', slot: 'antifreeze', next_mileage: null },
      { service_id: 's4', slot: 'gear_oil', next_mileage: 257000 },
    ])
    expect(nextTooLow(rows, '217000')).toBeNull()
    const low = nextRows({ sources, extra: [], edits: { oil: '216000' }, mileage: '217000', unit: 'km', own: {} })
    expect(nextTooLow(low, '217000')?.key).toBe('oil')
  })

  it('пробег не вписан (касса без сети) — пустое поле цели не даёт; очищенное кассиром — «не напоминать» (ADR-131)', () => {
    const sources = cartSources([{ service: oil }, { service: anti }, { service: gur }])
    const rows = nextRows({ sources, extra: ['s6'], edits: { antifreeze: '' }, mileage: '', unit: 'km', own: {} })
    expect(rows.map((r) => [r.key, r.value, r.edited])).toEqual([
      ['oil', '', false],
      ['antifreeze', '', true],
      ['svc:s6', '', false],
    ])
    expect(nextPayload(rows)).toEqual([{ service_id: 's2', slot: 'antifreeze', next_mileage: null }])
    // Кассир вписал число сам — уходит и без пробега.
    const typed = nextRows({ sources, extra: ['s6'], edits: { oil: '225000', 'svc:s6': '300000' }, mileage: '', unit: 'km', own: {} })
    expect(nextPayload(typed)).toEqual([
      { service_id: 's1', slot: 'oil', next_mileage: 225000 },
      { service_id: 's6', slot: 'other', next_mileage: 300000 },
    ])
  })

  it('мили: интервал в км переводится в мили машины', () => {
    const rows = nextRows({ sources: cartSources([{ service: oil }]), extra: [], edits: {}, mileage: '60000', unit: 'mi', own: {} })
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

  it('«+ ещё строку» — только услуги «прочее» этого чека; товары — никогда (ADR-131)', () => {
    // Масло — строкой по услуге, воздушный фильтр — на тот же пробег; ATF и антифриз без своих услуг —
    // доливка: ни строк, ни «+ ещё».
    const rows = rowsOf(sample)
    expect(rows.map((r) => [r.key, r.label, r.value])).toEqual([['oil', 'Замена со своим маслом', '258000']])
    expect(rows[0].filters).toEqual(['Фильтр воздушный MANN C-110'])
    expect(extraChoices(cartSources(sample), rows).map((c) => c.label)).toEqual(['Замена ГУР', 'Смазка кардана, 1 точка'])
    // Выбрали «Замена ГУР» — строка с её названием; интервала у неё нет — поле пустое. У кардана
    // интервал задан — заполнено заранее.
    const withGur = rowsOf(sample, ['s6', 's7'])
    expect(withGur.map((r) => [r.key, r.label, r.value, r.extra, r.interval_days])).toEqual([
      ['oil', 'Замена со своим маслом', '258000', false, 31],
      ['svc:s6', 'Замена ГУР', '', true, null],
      ['svc:s7', 'Смазка кардана, 1 точка', '270000', true, 180],
    ])
    expect(extraChoices(cartSources(sample), withGur)).toEqual([])
    // ГУР вписали — уходит услугой «прочее»; пустая строка «+ ещё» не уходит.
    const typed = rowsOf(sample, ['s6'], { 'svc:s6': '400000' })
    expect(nextPayload(typed)).toEqual([
      { service_id: 's5', slot: 'oil', next_mileage: 258000 },
      { service_id: 's6', slot: 'other', next_mileage: 400000 },
    ])
    expect(nextPayload(rowsOf(sample, ['s6']))).toEqual([{ service_id: 's5', slot: 'oil', next_mileage: 258000 }])
    // Услуги «прочее» нет в чеке — строки нет, хоть её и добавляли; доливка на вынос не предлагается.
    expect(rowsOf([{ service: ownOil }], ['s6']).map((r) => r.key)).toEqual(['oil'])
    expect(extraChoices(cartSources([{ service: oil }, { service: { ...wash, takeaway: true } }, { product: plug }]), [])).toEqual([])
    // Две строки одной услуги — одна строка «+ ещё».
    expect(extraChoices(cartSources([{ service: gur }, { service: gur }]), []).map((c) => c.service_id)).toEqual(['s6'])
  })

  it('фильтры с заменой масла двигателя — без своих строк, на пробег масла (ADR-117)', () => {
    const cabin = product('p8', 'Фильтр салонный CU-2', 'c-filters', { filter_type: 'салонный' })
    const fuel = product('p9', 'Фильтр топливный WK-842', 'c-filters', { filter_type: 'топливный' })
    const lines = [{ product: fuel }, { product: cabin }, { product: oilFilter }, { service: svc('s9', 'Замена воздушного фильтра', 'air_filter', 15000) }, { service: oil }]
    const rows = rowsOf(lines, [], { air_filter: '230000' })
    // Услуга воздушного фильтра своей строки не даёт; топливный фильтр — товар без услуги: строки нет.
    expect(rows.map((r) => [r.key, r.value])).toEqual([['oil', '258000']])
    // Фильтр товаром — его подписью (задача 003 Н5), услуга без товара — видом.
    expect(filtersNote(rows[0])).toBe('и фильтры: Фильтр W712 (масляный), воздушный, Фильтр салонный CU-2 на тот же пробег')
    expect(nextPayload(rows)).toEqual([{ service_id: 's1', slot: 'oil', next_mileage: 258000 }])
    expect(filtersNote({ filters: ['воздушный'] })).toBe('и фильтр: воздушный на тот же пробег')
    expect(filtersNote({})).toBe('')
    // Без масла двигателя услуга фильтра — своей строкой.
    expect(rowsOf([{ product: airFilter }, { service: svc('s9', 'Замена воздушного фильтра', 'air_filter', 15000) }]).map((r) => [r.key, r.value])).toEqual([['air_filter', '265000']])
  })

  it('следующая замена дальше пробега сейчас на 200 000 и больше — подсказка, не запрет (ADR-131)', () => {
    const sources = cartSources([{ service: oil }, { service: gur }])
    const far = nextRows({ sources, extra: ['s6'], edits: { 'svc:s6': '417000' }, mileage: '217000', unit: 'km', own: {} })
    expect(nextTooFar(far, '217000')).toEqual({ row: far[1], by: 200_000 })
    expect(nextTooLow(far, '217000')).toBeNull()
    const near = nextRows({ sources, extra: ['s6'], edits: { 'svc:s6': '416999' }, mileage: '217000', unit: 'km', own: {} })
    expect(nextTooFar(near, '217000')).toBeNull()
    expect(nextTooFar(far, '')).toBeNull()
  })

  it('«Это замена?» — канистра моторного масла без услуги масла двигателя (ADR-117)', () => {
    const enough = new Set(['c-oil'])
    expect(oilChangeHint([{ product: motorOil }, { service: wash }], categories, enough)).toBe(true)
    expect(oilChangeHint([{ product: motorOil }, { service: oil }], categories, enough)).toBe(false)
    // Доливка на вынос — не замена масла: подсказка остаётся.
    expect(oilChangeHint([{ product: motorOil }, { service: topup }], categories, enough)).toBe(true)
    // Меньше канистры — доливка, подсказки нет; масло коробки — не моторное.
    expect(oilChangeHint([{ product: motorOil }], categories, new Set())).toBe(false)
    expect(oilChangeHint([{ product: motorOil }], [cat('c-oil', 'gear_oil')], enough)).toBe(false)
  })

  it('убрали услугу из чека — её строка «+ ещё» и правка кассира уходят', () => {
    const before = cartSources(sample)
    expect(pruneNext(['s6'], { oil: '259000', 'svc:s6': '300000' }, before)).toBeNull()
    const noGur = cartSources(sample.filter((l) => l.service !== gur))
    expect(pruneNext(['s6'], { oil: '259000', 'svc:s6': '300000' }, noGur)).toEqual({ extra: [], edits: { oil: '259000' } })
    // Убрали услугу масла — правка по строке масла тоже уходит; товары чека строк не держат.
    const noOil = cartSources(sample.filter((l) => l.service !== ownOil))
    expect(pruneNext(['s6'], { oil: '259000', air_filter: '1', 'svc:s6': '' }, noOil)).toEqual({ extra: ['s6'], edits: { 'svc:s6': '' } })
  })

  it('черновик и отложенный чек: строки «+ ещё» — услуги «прочее»; правки — по строкам', () => {
    expect(extraServices(['s6', 7, '', null])).toEqual(['s6'])
    expect(extraServices(undefined)).toEqual([])
    expect(nextEditsOf({ oil: '225000', 'svc:s6': '', bad: 5 })).toEqual({ oil: '225000', 'svc:s6': '' })
    expect(nextEditsOf(['oil'])).toEqual({})
    expect(nextEditsOf(null)).toEqual({})
    expect(serviceKey('s6')).toBe('svc:s6')
  })

  it('«Исправить»: следующие замены исходного чека — правками и строками «+ ещё» (ADR-131)', () => {
    const next = nextFromDraft([
      { service_id: 's5', slot: 'oil', next_mileage: 258000 },
      { service_id: 's4', slot: 'gear_oil', next_mileage: null },
      { service_id: 's6', slot: 'other', next_mileage: 290000 },
      { service_id: null, slot: 'other', next_mileage: 1 },
      { service_id: 's5', slot: 'oil', next_mileage: 1 },
      { service_id: 'x', slot: 'toString', next_mileage: 1 },
    ])
    expect(next).toEqual({ edits: { oil: '258000', gear_oil: '', 'svc:s6': '290000' }, extra: ['s6'] })
    const rows = rowsOf([{ service: ownOil }, { service: gear }, { service: gur }], next.extra, next.edits)
    expect(nextPayload(rows)).toEqual([
      { service_id: 's5', slot: 'oil', next_mileage: 258000 },
      { service_id: 's4', slot: 'gear_oil', next_mileage: null },
      { service_id: 's6', slot: 'other', next_mileage: 290000 },
    ])
    expect(nextFromDraft(undefined)).toEqual({ edits: {}, extra: [] })
  })

  it('снимок кассы: категории со строкой книжки; старый снимок — как не загружены', () => {
    expect(snapshotCategories([{ id: 'c1', book_slot: 'air_filter', book_interval_km: 15000, book_interval_days: 365 }, { id: 'c2', book_slot: 'none' }])).toEqual([
      { ...cat('c1', 'air_filter', 15000, 365), by_type: true, slot_by_name: false },
      { ...cat('c2', 'none'), by_type: true, slot_by_name: false },
    ])
    // «Тип» выключен у категории (задача 002 П10) — признак из снимка.
    expect(snapshotCategories([{ id: 'c3', book_slot: 'oil_filter', by_type: false }])?.[0]?.by_type).toBe(false)
    expect(snapshotCategories([{ id: 'c1' }])).toBeNull()
    expect(snapshotCategories(undefined)).toBeNull()
  })

  it('книжка для чека из очереди — как отдаст сервер: подпись товаром, вид замены, фильтры и «прочее» (задача 003 Н5)', () => {
    const lines = [
      { service: oil },
      { service: anti },
      { service: gur },
      { product: airFilter, kind: 'piece', qtyText: '1' },
      { product: antifreeze, kind: 'piece', qtyText: '2' },
      { product: { ...motorOil, id: 'p10', container_ml: 4000 }, kind: 'container', qtyText: '1' },
      { product: { ...motorOil, id: 'p11', name: 'Totachi 5W-30 розлив' }, kind: 'pour', qtyText: '0,5' },
    ]
    const rows = nextRows({ sources: cartSources(lines), goods: cartGoods(lines, categories), extra: ['s6'], edits: { antifreeze: '', 'svc:s6': '300000' }, mileage: '217000', unit: 'km', own: {} })
    const b = receiptBook({ brand: 'Toyota', model: 'Camry', plate: '01KG123ABC' }, '217000', 'km', rows, '2026-10-26', cartGoods(lines, categories))
    const motor = 'Замена масла в моторе'
    // Два товара строки масла — оба; воздушный фильтр — под заменой масла, на её пробег; ГУР — своим
    // названием и сроком; у антифриза «не напоминать» — без следующей замены.
    expect(b.lines).toEqual([
      { label: 'Totachi 5W-30 4 л · 1 кан. × 4 л, Totachi 5W-30 розлив · 0,5 л', slot: 'oil', service_id: null, group: motor, next_mileage: 225000, unit: 'km', next_date: '2026-11-26' },
      { label: 'Замена ГУР', slot: 'other', service_id: 's6', group: 'Замена ГУР', next_mileage: 300000, unit: 'km', next_date: null },
      { label: 'Фильтр воздушный MANN C-110 · 1 шт', slot: 'air_filter', service_id: null, group: motor, next_mileage: 225000, unit: 'km', next_date: '2026-11-26' },
    ])
    expect(b.filled?.map((f) => [f.label, f.group])).toEqual([
      ['Totachi 5W-30 4 л · 1 кан. × 4 л', motor],
      ['Totachi 5W-30 розлив · 0,5 л', motor],
      ['Felix G12 5 кг · 2 шт', 'Замена антифриза'],
      ['Замена ГУР', 'Замена ГУР'],
      ['Фильтр воздушный MANN C-110 · 1 шт', motor],
    ])
    expect(b.mileage).toBe(217000)
    // Настройка «только масла и жидкости» — без фильтра.
    const fluids = receiptBook({ brand: '', model: '', plate: '1' }, '217000', 'km', rows, '2026-10-26', cartGoods(lines, categories), { withFilters: false })
    expect(fluids.filled?.map((f) => f.slot)).toEqual(['oil', 'oil', 'antifreeze', 'other'])
    // Масло клиента — услуга без товара.
    const own = nextRows({ sources: cartSources([{ service: oil }]), extra: [], edits: {}, mileage: '217000', unit: 'km', own: {} })
    expect(receiptBook({ brand: '', model: '', plate: '1' }, '217000', 'km', own, '2026-10-26').filled).toEqual([
      { slot: 'oil', label: 'масло клиента (двигатель)', product: 'масло клиента', service_id: null, group: motor },
    ])
    // Только «прочее» — работа своим названием и сроком.
    const other = nextRows({ sources: cartSources([{ service: gur }]), extra: ['s6'], edits: { 'svc:s6': '300000' }, mileage: '217000', unit: 'km', own: {} })
    expect(receiptBook({ brand: '', model: '', plate: '1' }, '217000', 'km', other, '2026-10-26').lines.map((l) => [l.label, l.next_mileage])).toEqual([['Замена ГУР', 300000]])
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
    // ADR-131: класс жидкости без слова «тормозная» / «антифриз» — те же примеры, что на сервере.
    for (const name of ['FENIX DOT 4', 'DOT-4 MANNOL 0.5', 'Unix Dot4', 'ABRO dot3 354 мл', 'Motul DOT 5.1 0,5 л', 'ROSDOT 4 455 г', 'Жидкость ДОТ-4 Felix']) {
      expect(nameSlot(name), name).toBe('brake_fluid')
    }
    for (const name of ['SIBIRIA GREEN G11', 'Sintec G12+', 'Felix Carbox g12++ 5 кг', 'Coolstream G13 1 кг', 'Hepu P999-G12']) {
      expect(nameSlot(name), name).toBe('antifreeze')
    }
    for (const name of ['Омыватель стекла -20', 'Щётка G-Force 600 мм', 'G-Energy Expert G 10W-40 4 л', 'G-Box ATF DX III', 'Лампа G4 12V', 'Кольцо SG12', 'Артикул G1205', 'Набор Dotted 3 шт', 'DOT 45 мм']) {
      expect(nameSlot(name), name).toBeNull()
    }
    // Строка категории важнее названия; «прочее» — не жидкость.
    expect(goodSlot(cat('c-misc', 'other'), { name: 'Антифриз G12', attrs: {} })).toBe('other')
    expect(goodSlot(undefined, { name: 'Антифриз G12', attrs: {} })).toBeNull()
    // Рабочая база: «Антифриз и жидкости» без строки книжки + услуга «Замена антифриза».
    const sintec = product('p20', 'Антифриз Sintec G12 красный 5 кг', 'c-plugs')
    const lines = [{ product: sintec, kind: 'piece', qtyText: '1' }, { service: anti }]
    const goods = cartGoods(lines, categories)
    expect(goods.map((g) => g.slot)).toEqual(['antifreeze'])
    const rows = nextRows({ sources: cartSources(lines), extra: [], edits: {}, mileage: '120000', unit: 'km', own: {} })
    const b = receiptBook({ brand: '', model: '', plate: '1' }, '120000', 'km', rows, '2026-10-27', goods)
    expect(b.filled).toEqual([{ slot: 'antifreeze', label: 'Антифриз Sintec G12 красный 5 кг · 1 шт', product: 'Антифриз Sintec G12 красный 5 кг · 1 шт', service_id: null, group: 'Замена антифриза' }])
  })

  it('проданное масло без услуги своей строки — замена; доливка этой строки, ГУР и мост — нет (ADR-136)', () => {
    const sold = (lines: Parameters<typeof cartSources>[0]) => soldSources(lines, categories)
    const rows = (lines: Parameters<typeof cartSources>[0], edits: NextEdits = {}) =>
      nextRows({ sources: [...cartSources(lines), ...sold(lines)], extra: [], edits, mileage: '130000', unit: 'km', own: {} })
    // Чек № 17: ATF и «Замена со своим маслом» — строка коробки по интервалу по умолчанию.
    const r17 = rows([{ product: atf }, { service: ownOil }])
    expect(r17.map((r) => [r.key, r.service_id, r.value])).toEqual([
      ['oil', 's5', '138000'],
      ['gear_oil', '', '170000'],
    ])
    // Подпись строки по товару — товар, а не «Масло в коробке» (задача 003 Н5).
    expect(r17[1].label).toBe('ATF Dexron III')
    // Цель по строке без услуги — `service_id: null`; очищенная — «не напоминать».
    expect(nextPayload(rows([{ product: atf }, { service: wash }], { gear_oil: '' }))).toEqual([{ service_id: null, slot: 'gear_oil', next_mileage: null }])
    expect(nextPayload(rows([{ product: motorOil }, { service: wash }]))).toEqual([{ service_id: null, slot: 'oil', next_mileage: 138000 }])
    // Объём не важен; подарок — не замена; антифриз без услуги — доливка (ADR-117).
    expect(sold([{ product: atf, kind: 'pour', qtyText: '0.5' }]).map((x) => x.slot)).toEqual(['gear_oil'])
    expect(sold([{ product: atf, gift: true }, { product: antifreeze }])).toEqual([])
    // Доливка коробки гасит ATF, но не масло двигателя; доливка масла — масло двигателя.
    const kpp = { ...svc('s9', 'Доливка КПП снизу', 'other', null), takeaway: true }
    expect(sold([{ product: atf }, { product: motorOil }, { service: kpp }]).map((x) => x.slot)).toEqual(['oil'])
    expect(sold([{ product: motorOil }, { service: topup }])).toEqual([])
    // ATF при ГУР и при мосте — не замена коробки (ADR-131).
    const bridge = svc('s10', 'Замена масла мех. моста', 'other', null)
    expect(sold([{ product: atf }, { service: gur }])).toEqual([])
    expect(sold([{ product: atf }, { service: bridge }])).toEqual([])
    // Услуга своей строки уже даёт строку — по товару второй нет.
    expect(sold([{ product: atf }, { service: gear }])).toEqual([])
    // Правка строки по товару переживает изменение услуг: строка есть в источниках.
    const sources = [...cartSources([{ product: atf }, { service: wash }]), ...sold([{ product: atf }, { service: wash }])]
    expect(pruneNext([], { gear_oil: '175000' }, sources)).toBeNull()
    // «Исправить»: цель по товару (без услуги) возвращается правкой строки.
    expect(nextFromDraft([{ service_id: null, slot: 'gear_oil', next_mileage: 175000 }]).edits).toEqual({ gear_oil: '175000' })
  })

  it('«Тип» фильтра выключен у категории — строка книжки от категории (задача 002 П10)', () => {
    expect(productSlot('oil_filter', 'воздушный', false)).toBe('oil_filter')
    expect(goodSlot({ book_slot: 'oil_filter', by_type: false }, airFilter)).toBe('oil_filter')
    expect(goodSlot({ book_slot: 'oil_filter' }, airFilter)).toBe('air_filter')
  })

  it('фильтр без типа в категории фильтров без выбранной строки — по названию товара (задача 003 Н5)', () => {
    const fuel = { name: 'Фильтр топливный WK 820', attrs: {} }
    expect(goodSlot({ book_slot: 'oil_filter', slot_by_name: true }, fuel)).toBe('fuel_filter')
    expect(goodSlot({ book_slot: 'oil_filter', slot_by_name: true, by_type: false }, fuel)).toBe('fuel_filter')
    expect(goodSlot({ book_slot: 'oil_filter', slot_by_name: true }, { name: 'Фильтр W920', attrs: {} })).toBe('oil_filter')
    // Тип решает раньше названия; строку выбрал владелец — название не решает.
    expect(goodSlot({ book_slot: 'oil_filter', slot_by_name: true }, { name: 'Фильтр топливный', attrs: { filter_type: 'масляный' } })).toBe('oil_filter')
    expect(goodSlot({ book_slot: 'oil_filter', slot_by_name: false }, fuel)).toBe('oil_filter')
    expect(goodSlot({ book_slot: 'oil_filter' }, fuel)).toBe('oil_filter')
  })

  it('канистра и разлив одного масла — одним товаром: «4,5 л», с разбивкой по настройке (задача 003 Н5)', () => {
    const can4 = { ...motorOil, unit: 'ml', container_ml: 4000 } as Product
    const lines = [
      { product: can4, kind: 'container', qtyText: '1' },
      { product: can4, kind: 'pour', qtyText: '0,5' },
      { product: antifreeze, kind: 'piece', qtyText: '1' },
      { product: antifreeze, kind: 'piece', qtyText: '2' },
    ]
    const goods = cartGoods(lines, categories)
    expect(goods.map((g) => g.slot)).toEqual(['oil', 'antifreeze'])
    expect(goodBook(goods[0])).toBe('Totachi 5W-30 4 л · 4,5 л')
    expect(goodBook(goods[0], 'split')).toBe('Totachi 5W-30 4 л · 4,5 л (1 кан. × 4 л + 0,5 л)')
    expect(goodShort(goods[0])).toBe('Totachi 5W-30 4 л × 4,5 л')
    expect(goodBook(goods[1])).toBe('Felix G12 5 кг · 3 шт')
    // Одна строка — как было.
    expect(goodBook(cartGoods([{ product: can4, kind: 'container', qtyText: '2' }], categories)[0])).toBe('Totachi 5W-30 4 л · 2 кан. × 4 л')
    // На вес и в длину — кг и м, не «шт» (ADR-175).
    const hose = { slot: 'oil' as const, name: 'Шланг', kind: 'length', qty: '1,5', containerMl: null }
    expect(goodBook(hose)).toBe('Шланг · 1,5 м')
    expect(goodShort({ ...hose, kind: 'weight' })).toBe('Шланг × 1,5 кг')
    expect(goodBook({ ...hose, parts: [{ kind: 'length', qty: '1' }, { kind: 'length', qty: '0,5' }] })).toBe('Шланг · 1 м + 0,5 м')
    const rows = nextRows({ sources: cartSources([{ service: oil }]), goods, extra: [], edits: {}, mileage: '130000', unit: 'km', own: {} })
    const book = receiptBook({ brand: 'Toyota', model: 'Camry', plate: '01KG' }, '130000', 'km', rows, '2026-10-31', goods)
    expect(book.filled).toEqual([{ slot: 'oil', label: 'Totachi 5W-30 4 л · 4,5 л', product: 'Totachi 5W-30 4 л · 4,5 л', service_id: null, group: 'Замена масла в моторе' }])
  })

  it('фильтры с маслом — своими строками, если владелец выключил «на пробег масла»; чек книжки с фильтрами', () => {
    const lines = [{ service: oil }, { product: airFilter }, { product: oilFilter }]
    const on = nextRows({ sources: cartSources(lines), goods: cartGoods(lines, categories), extra: [], edits: {}, mileage: '130000', unit: 'km', own: {} })
    expect(on.map((r) => r.slot)).toEqual(['oil'])
    expect(on[0].filters).toEqual(['Фильтр W712 (масляный)', 'Фильтр воздушный MANN C-110'])
    const off = nextRows({ sources: cartSources(lines), goods: cartGoods(lines, categories), extra: [], edits: {}, mileage: '130000', unit: 'km', own: {}, filtersFollowOil: false })
    expect(off[0].filters).toBeUndefined()
    const book = receiptBook({ brand: '', model: '', plate: '01KG' }, '130000', 'km', on, '2026-10-31', cartGoods(lines, categories), { withFilters: true })
    expect(book.lines.map((l) => [l.slot, l.next_mileage])).toEqual([
      ['oil', 138000],
      ['air_filter', 138000],
      ['oil_filter', 138000],
    ])
    expect(book.filled?.map((f) => f.product)).toEqual(['масло клиента', 'Фильтр воздушный MANN C-110 · 1 шт', 'Фильтр W712 · 1 шт'])
  })

  it('топливный фильтр в чеке из очереди — свой срок, не пробег масла (задача 003 Н5, ADR-117)', () => {
    const fuel = product('p9', 'Фильтр топливный WK-842', 'c-filters', { filter_type: 'топливный' })
    const lines = [{ service: oil }, { product: oilFilter }, { product: fuel }]
    const goods = cartGoods(lines, categories)
    expect(goods.map((g) => g.slot)).toEqual(['oil_filter', 'fuel_filter'])
    const rows = nextRows({ sources: cartSources(lines), goods, extra: [], edits: {}, mileage: '130000', unit: 'km', own: {} })
    const book = receiptBook({ brand: '', model: '', plate: '01KG' }, '130000', 'km', rows, '2026-10-31', goods)
    // Масляный — с маслом (138 000), топливный — 30 000 км по умолчанию (160 000), без вида замены.
    expect(book.lines.map((l) => [l.label, l.next_mileage, l.group])).toEqual([
      ['масло клиента (двигатель)', 138000, 'Замена масла в моторе'],
      ['Фильтр W712 (масляный) · 1 шт', 138000, 'Замена масла в моторе'],
      ['Фильтр топливный WK-842 · 1 шт', 160000, null],
    ])
    // Свой интервал машины — по нему; с услугой топливного фильтра — по её строке и под ней.
    const own = receiptBook({ brand: '', model: '', plate: '01KG' }, '130000', 'km', rows, '2026-10-31', goods, { own: { fuel_filter: { km: 45000, days: 730 } } })
    expect(own.lines.find((l) => l.slot === 'fuel_filter')?.next_mileage).toBe(175000)
    const fuelSvc = svc('s12', 'Замена топливного фильтра', 'fuel_filter', 30000, 730)
    const withSvc = [...lines, { service: fuelSvc }]
    const rows2 = nextRows({ sources: cartSources(withSvc), goods, extra: [], edits: { fuel_filter: '161000' }, mileage: '130000', unit: 'km', own: {} })
    expect(rows2.map((r) => r.slot)).toEqual(['oil', 'fuel_filter'])
    const book2 = receiptBook({ brand: '', model: '', plate: '01KG' }, '130000', 'km', rows2, '2026-10-31', goods)
    expect(book2.lines.find((l) => l.slot === 'fuel_filter')).toMatchObject({ next_mileage: 161000, group: 'Замена топливного фильтра' })
  })
})
