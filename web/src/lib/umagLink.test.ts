import { describe, expect, it, vi } from 'vitest'
import { ApiError } from './api'
import { findByBarcode, searchSnapshot, type Snapshot, type SnapshotProduct } from './offline'
import type { OldCatalogHit, Product } from './types'
import {
  codesShort,
  conflictText,
  hitCodes,
  hitMeasureCodes,
  hitProductCodes,
  isMeasureUnit,
  linkSummary,
  opIds,
  resolveCode,
  sameCode,
  similarityText,
  umagCodes,
  umagPer,
  umagUnitWord,
  umagStep,
  unlinkedHit,
  type CodeLookup,
} from './umagLink'

const hit = (over: Partial<OldCatalogHit>): OldCatalogHit => ({
  id: 'u1',
  name: 'Масло ZIC X7 5W-30 4L',
  barcode: '8801111111111',
  article: 'ZIC-X7',
  cost_tyiyn: 180_000,
  price_tyiyn: 260_000,
  unit: 'Литр',
  supplier: '',
  category: '',
  exact: true,
  linked_product_id: null,
  product_id: null,
  product_name: null,
  codes: ['8801111111111', 'ZIC-X7'],
  conflicts: [],
  ...over,
})

const product = (id: string, barcodes: string[] = []): SnapshotProduct =>
  ({ id, name: `Товар ${id}`, brand: '', article: '', barcodes }) as unknown as SnapshotProduct

describe('коды строки Умага — как на сервере (SPEC-35 §1)', () => {
  it('штрихкод, артикул-код и каждый доп. код через «;», «,» и пробел', () => {
    expect(umagCodes({ barcode: '4600000000017', article: 'W712', extra_code: '1111; 2222,08227-99974  692303BOSN770' })).toEqual([
      '4600000000017',
      'W712',
      '1111',
      '2222',
      '08227-99974',
      '692303BOSN770',
    ])
  })

  it('артикул — код, только если в нём латиница, цифры или дефис, от четырёх знаков и есть цифра', () => {
    expect(umagCodes({ barcode: '1', article: 'OC-90', extra_code: '' })).toEqual(['1', 'OC-90'])
    expect(umagCodes({ barcode: '1', article: 'OC9', extra_code: '' })).toEqual(['1'])
    expect(umagCodes({ barcode: '1', article: 'W 712/75', extra_code: '' })).toEqual(['1'])
    expect(umagCodes({ barcode: '1', article: 'Фильтр', extra_code: '' })).toEqual(['1'])
    expect(umagCodes({ barcode: '1', article: 'CITY', extra_code: '' })).toEqual(['1'])
  })

  it('из доп. кода — только куски, похожие на код: слова, знаки, вязкость, объём и обрывки — не коды', () => {
    // Рабочая выгрузка 26.10.2026.
    expect(umagCodes({ barcode: '2760111', article: '', extra_code: 'SHELLHELIX CITY 5W30 1L' })).toEqual(['2760111'])
    expect(umagCodes({ barcode: '4', article: '', extra_code: '0888602905  -;0888602905' })).toEqual(['4', '0888602905'])
    expect(umagCodes({ barcode: '4', article: '', extra_code: '0888602305  U' })).toEqual(['4', '0888602305'])
    expect(umagCodes({ barcode: '4', article: '', extra_code: '1156;0888681015  Z' })).toEqual(['4', '1156', '0888681015'])
    expect(umagCodes({ barcode: '4', article: '', extra_code: '483;4607095382491' })).toEqual(['4', '4607095382491'])
    expect(umagCodes({ barcode: '4', article: '', extra_code: '4673726890117;46' })).toEqual(['4', '4673726890117'])
    expect(umagCodes({ barcode: '4', article: '', extra_code: '10W-40 JLE0119101 13479184-00' })).toEqual(['4', 'JLE0119101', '13479184-00'])
    expect(umagCodes({ barcode: '4', article: '', extra_code: 'П20220820412' })).toEqual(['4'])
  })

  it('строка «литр»: коды Умага короче восьми цифр (276…, PLU) товару не переходят', () => {
    const liter = hit({ unit: 'литр', codes: ['2760391', '1392', '4008177071423', '08227-99974'] })
    expect(hitProductCodes(liter)).toEqual(['4008177071423', '08227-99974'])
    expect(hitMeasureCodes(liter)).toEqual(['2760391', '1392'])
    expect(hitProductCodes(hit({ unit: 'шт', codes: ['2760391', '1392'] }))).toEqual(['2760391', '1392'])
    expect(isMeasureUnit('литр')).toBe(true)
    expect(isMeasureUnit('Кг')).toBe(true)
    expect(isMeasureUnit('метр')).toBe(true)
    expect(isMeasureUnit('шт')).toBe(false)
    expect(isMeasureUnit('')).toBe(false)
  })

  it('пустые и повторы выкидываются, порядок — как в строке', () => {
    expect(umagCodes({ barcode: '  ', article: '', extra_code: ' ; ,' })).toEqual([])
    expect(umagCodes({ barcode: '4600000000017', article: '4600000000017', extra_code: '4600000000017;5000;5000' })).toEqual([
      '4600000000017',
      '5000',
    ])
    expect(umagCodes({ barcode: ' 77 ', article: '' })).toEqual(['77'])
  })

  it('коды найденной строки — от сервера; у прежнего ответа — штрихкод и артикул-код', () => {
    expect(hitCodes(hit({ codes: ['1', '2', '3'] }))).toEqual(['1', '2', '3'])
    expect(hitCodes(hit({ codes: undefined }))).toEqual(['8801111111111', 'ZIC-X7'])
    expect(codesShort(hit({ codes: ['1', '2', '3', '4', '5'] }))).toBe('коды 1, 2, 3 и ещё 2')
    expect(codesShort(hit({ codes: ['1'] }))).toBe('код 1')
    expect(codesShort(hit({ codes: [] }))).toBe('')
  })
})

describe('тот же код без потерянных нулей', () => {
  it('числовой код совпадает и без нулей впереди, буквенный — только как есть', () => {
    expect(sameCode('000123456', '123456')).toBe(true)
    expect(sameCode('123456', '0123456')).toBe(true)
    expect(sameCode(' 42 ', '42')).toBe(true)
    expect(sameCode('0OC90', 'OC90')).toBe(false)
    expect(sameCode('123', '1234')).toBe(false)
    expect(sameCode('', '')).toBe(false)
    expect(sameCode('000', '0')).toBe(false)
  })

  it('касса без сети находит товар по коду Умага с потерянными нулями (снимок, SPEC-35 §1)', () => {
    const snap = { products: [product('a', ['2000000000015']), product('b', ['123456', 'ZIC-X7'])] } as unknown as Snapshot
    expect(findByBarcode(snap, '000123456')?.id).toBe('b')
    expect(findByBarcode(snap, 'ZIC-X7')?.id).toBe('b')
    expect(findByBarcode(snap, '2000000000015')?.id).toBe('a')
    expect(findByBarcode(snap, '999')).toBeUndefined()
    expect(searchSnapshot(snap, '0123456').map((p) => p.id)).toEqual(['b'])
  })

  it('точное совпадение важнее совпадения без нулей', () => {
    const snap = { products: [product('a', ['123']), product('b', ['0123'])] } as unknown as Snapshot
    expect(findByBarcode(snap, '0123')?.id).toBe('b')
    expect(findByBarcode(snap, '00123')?.id).toBe('a')
  })
})

describe('строки Умага в поиске', () => {
  it('блок Умага показывает только не заведённые: без связи и без своего кода у товара', () => {
    expect(unlinkedHit(hit({}))).toBe(true)
    expect(unlinkedHit(hit({ product_id: 'p1' }))).toBe(false)
    expect(unlinkedHit(hit({ linked_product_id: 'p2' }))).toBe(false)
    // Прежний ответ сервера без связи.
    expect(unlinkedHit(hit({ product_id: undefined }))).toBe(true)
  })
})

describe('скан кода, которого нет у товаров (SPEC-35 §2)', () => {
  it('связанная строка — её товар; несвязанная — окно Умага; нет точного совпадения — «Код не найден»', () => {
    expect(umagStep([hit({ product_id: 'p1' })])).toMatchObject({ kind: 'linked', productId: 'p1' })
    expect(umagStep([hit({ id: 'u2' })])).toMatchObject({ kind: 'umag', hit: { id: 'u2' } })
    expect(umagStep([hit({ exact: false })])).toEqual({ kind: 'none' })
    expect(umagStep([])).toEqual({ kind: 'none' })
  })

  it('тот же код у двух строк: связанная важнее, похожие по названию не в счёт', () => {
    const step = umagStep([hit({ id: 'free' }), hit({ id: 'similar', exact: false, product_id: 'x' }), hit({ id: 'linked', product_id: 'p9' })])
    expect(step).toMatchObject({ kind: 'linked', productId: 'p9', hit: { id: 'linked' } })
  })

  const notFound = () => new ApiError(404, 'not_found', 'не найдено')
  const lookup = (over: Partial<CodeLookup>): CodeLookup => ({
    byBarcode: vi.fn(async () => {
      throw notFound()
    }),
    searchOwn: vi.fn(async () => []),
    searchUmag: vi.fn(async () => []),
    productById: vi.fn(async (id: string) => ({ id }) as Product),
    ...over,
  })

  it('код товара — сам товар, Умаг не спрашиваем', async () => {
    const api = lookup({ byBarcode: vi.fn(async () => ({ id: 'own' }) as Product) })
    expect(await resolveCode('4600000000017', api)).toEqual({ kind: 'product', product: { id: 'own' }, via: 'code' })
    expect(api.searchUmag).not.toHaveBeenCalled()
  })

  it('нет у товаров, строка Умага связана — берём связанный товар', async () => {
    const api = lookup({ searchUmag: vi.fn(async () => [hit({ product_id: 'p7' })]) })
    expect(await resolveCode('8801111111111', api)).toEqual({ kind: 'product', product: { id: 'p7' }, via: 'umag' })
    expect(api.searchUmag).toHaveBeenCalledWith('8801111111111')
    expect(api.productById).toHaveBeenCalledWith('p7')
  })

  it('нет у товаров, строка не связана — окно Умага; нет нигде — «Код не найден»', async () => {
    expect(await resolveCode('8801111111111', lookup({ searchUmag: vi.fn(async () => [hit({})]) }))).toMatchObject({ kind: 'umag', hit: { id: 'u1' } })
    expect(await resolveCode('999', lookup({}))).toEqual({ kind: 'unknown' })
  })

  it('артикул своего товара — сам товар, а не окно Умага «среди товаров его нет»', async () => {
    const own = { id: 'ngk', article: 'BKR6E' } as Product
    const api = lookup({
      searchOwn: vi.fn(async () => [own, { id: 'other', article: 'BKR6EIX' } as Product]),
      searchUmag: vi.fn(async () => [hit({ article: 'BKR6E' })]),
    })
    expect(await resolveCode('bkr6e', api)).toEqual({ kind: 'product', product: own, via: 'article' })
    expect(api.searchUmag).not.toHaveBeenCalled()
    // Тот же артикул у двух своих товаров — они в списке, окна нет.
    const two = lookup({ searchOwn: vi.fn(async () => [own, { id: 'ngk2', article: 'BKR6E' } as Product]) })
    expect(await resolveCode('BKR6E', two)).toMatchObject({ kind: 'own', products: [{ id: 'ngk' }, { id: 'ngk2' }] })
  })

  it('нет связи — ошибка наружу (касса без сети ищет в снимке), Умаг не спрашиваем', async () => {
    const api = lookup({
      byBarcode: vi.fn(async () => {
        throw new ApiError(0, 'network', 'Нет связи с сервером')
      }),
    })
    await expect(resolveCode('123', api)).rejects.toMatchObject({ status: 0 })
    expect(api.searchUmag).not.toHaveBeenCalled()
  })
})

describe('итоги связывания', () => {
  it('перенесённые коды и конфликты словами', () => {
    expect(linkSummary({ copied: ['1', '2'], conflicts: [] })).toBe('коды перенесены: 1, 2')
    expect(linkSummary({ copied: [], conflicts: [{ code: '3', product_id: 'p', product_name: 'Масло Х' }] })).toBe(
      'новых кодов нет; не перенесены — код 3 — у «Масло Х»',
    )
    expect(conflictText([])).toBe('')
  })

  it('цена строки меры в Умаге — за литр, кг, метр; у штучной — без приписки', () => {
    expect(umagPer('литр')).toBe(' за л')
    expect(umagPer('кг')).toBe(' за кг')
    expect(umagPer('метр')).toBe(' за м')
    expect(umagPer('шт')).toBe('')
    expect(umagUnitWord('Литр')).toBe('литр')
    expect(umagUnitWord('кг')).toBe('килограмм')
  })

  it('сходство названий — процентом', () => {
    expect(similarityText(0.734)).toBe('73 %')
    expect(similarityText(1.2)).toBe('100 %')
    expect(similarityText(-1)).toBe('0 %')
  })

  it('повтор того же действия — тот же op_id, после ответа — новый', () => {
    const ops = opIds()
    const first = ops.get('link:u1:p1')
    expect(ops.get('link:u1:p1')).toBe(first)
    expect(ops.get('link:u2:p1')).not.toBe(first)
    ops.done('link:u1:p1')
    expect(ops.get('link:u1:p1')).not.toBe(first)
  })
})
