import { describe, expect, it } from 'vitest'
import {
  costSummary,
  entryText,
  entryUnitPrice,
  invoiceEntry,
  markupText,
  otherPriceText,
  priceWithLiter,
  typed,
  unitCostOrNull,
} from './purchasePrice'

const nb = (s: string) => s.replace(/ /g, ' ')

describe('цена прихода «за кан. | за л» (задача 003, Н7)', () => {
  it('450 с за литр × 4 л → 1 800 с за канистру', () => {
    const e = typed('450', 'l')
    expect(entryUnitPrice(e, 4000)).toBe(180_000)
    expect(entryText(e, 'can', 4000)).toBe('1800')
    expect(nb(otherPriceText(e, 'l', 4000))).toBe('= 1 800 с за канистру 4 л')
  })

  it('1 999,99 с за канистру 4 л → 500 с за литр', () => {
    const e = typed('1999,99', 'can')
    expect(entryText(e, 'l', 4000)).toBe('500')
    expect(nb(otherPriceText(e, 'can', 4000))).toBe('= 500 с за л')
    expect(entryUnitPrice(e, 4000)).toBe(199_999)
  })

  it('переключение туда и обратно не накапливает ошибку: введённое хранится', () => {
    const e = typed('1999,99', 'can')
    const shownL = entryText(e, 'l', 4000)
    expect(shownL).toBe('500')
    // Не меняли — снова канистра, как ввели, а не 500 × 4 = 2000.
    expect(entryText(e, 'can', 4000)).toBe('1999,99')
    expect(entryUnitPrice(e, 4000)).toBe(199_999)
  })

  it('штука и кг — без перевода; пусто и ошибка', () => {
    expect(entryUnitPrice(typed('350', 'l'), null)).toBe(35_000)
    expect(entryText(typed('350', 'can'), 'l', null)).toBe('350')
    expect(otherPriceText(typed('350', 'can'), 'can', null)).toBe('')
    expect(entryUnitPrice(typed('  ', 'can'), 4000)).toBe('empty')
    expect(entryUnitPrice(typed('abc', 'can'), 4000)).toBeNull()
    expect(entryUnitPrice(typed('-5', 'can'), null)).toBeNull()
    expect(unitCostOrNull('empty')).toBeNull()
    expect(unitCostOrNull(100)).toBe(100)
  })

  it('наценка к цене продажи', () => {
    expect(nb(markupText(160_000, 200_000))).toBe('наценка к цене продажи: 25,0 %')
    expect(markupText('empty', 200_000)).toBe('')
    expect(markupText(0, 200_000)).toBe('')
  })

  it('строка накладной в карточку: цена и единица', () => {
    expect(invoiceEntry({ price_tyiyn: 45_000, sum_tyiyn: 900_000, qty_milli: 20_000, liters: true })).toEqual({ text: '450', per: 'l' })
    expect(invoiceEntry({ price_tyiyn: null, sum_tyiyn: 900_000, qty_milli: 20_000, liters: true })).toEqual({ text: '450', per: 'l' })
    // 3 шт на 1 000 с — 333,33 с за штуку, одно округление.
    expect(invoiceEntry({ price_tyiyn: null, sum_tyiyn: 100_000, qty_milli: 3000, liters: false })).toEqual({ text: '333,33', per: 'can' })
    expect(invoiceEntry({ price_tyiyn: null, sum_tyiyn: null, qty_milli: 3000, liters: false })).toBeUndefined()
  })
})

describe('себестоимость в карточке (задача 003, Н7)', () => {
  const oil = { unit: 'ml' as const, container_ml: 4000, sale_price_tyiyn: 200_000 }

  it('масло: средняя за канистру и за литр, стоимость остатка, наценка', () => {
    const s = costSummary({ ...oil, stock_qty: 34_000, avg_cost_tyiyn: 175_000, stock_value_tyiyn: 1_487_500, last_purchase_price_tyiyn: 180_000 })
    expect(nb(s.average ?? '')).toBe('1 750 с за канистру 4 л · 437,50 с за л')
    expect(nb(s.stockValue ?? '')).toBe('8 кан. + 2 л — 14 875 с')
    expect(nb(s.markup ?? '')).toBe('цена продажи 2 000 с — наценка 14,3 % к средней')
    expect(s.note).toBeNull()
    expect(s.askPrice).toBe(false)
  })

  it('без остатка — продажа по последней цене прихода', () => {
    const s = costSummary({ ...oil, stock_qty: 0, stock_value_tyiyn: 0, last_purchase_price_tyiyn: 180_000 })
    expect(s.average).toBeNull()
    expect(s.stockValue).toBeNull()
    expect(s.note).toBe('остатка нет — продажа пойдёт по последней цене прихода')
    expect(nb(s.markup ?? '')).toBe('цена продажи 2 000 с — наценка 11,1 % к последней закупке')
    expect(s.askPrice).toBe(false)
  })

  it('остаток без цены и товар без закупки — «Указать цену закупки»', () => {
    const surplus = costSummary({ ...oil, stock_qty: 4000, stock_value_tyiyn: 0, last_purchase_price_tyiyn: null })
    expect(surplus.note).toBe('остаток без цены закупки — укажите её, остаток оценится по ней')
    expect(surplus.askPrice).toBe(true)
    const none = costSummary({ unit: 'piece', container_ml: null, sale_price_tyiyn: 50_000, stock_qty: 0, stock_value_tyiyn: 0, last_purchase_price_tyiyn: null })
    expect(none.note).toBe('остатка нет и цены закупки нет')
    expect(none.askPrice).toBe(true)
  })

  it('администратору сервер не отдаёт среднюю и стоимость: блок пуст', () => {
    const s = costSummary({ ...oil, stock_qty: 8000, last_purchase_price_tyiyn: 180_000 })
    expect(s.average).toBeNull()
  })

  it('цена с литром: «1 800 с за канистру 4 л · 450 с за л», штука — без литра', () => {
    expect(nb(priceWithLiter(180_000, oil))).toBe('1 800 с за канистру 4 л · 450 с за л')
    expect(nb(priceWithLiter(35_000, { unit: 'piece', container_ml: null }))).toBe('350 с за шт')
    expect(nb(priceWithLiter(12_000, { unit: 'mm', container_ml: null }))).toBe('120 с за м')
    expect(nb(priceWithLiter(12_000, { unit: 'g', container_ml: null }))).toBe('120 с за кг')
  })

  it('кг и м без остатка: наценка к последней закупке за кг (м) — как её отдал сервер (задача 006)', () => {
    // Последняя закупка 100 с за м, продажа 150 с за м — наценка 50 %, а не ×1000.
    const s = costSummary({ unit: 'mm', container_ml: null, sale_price_tyiyn: 15_000, stock_qty: 0, stock_value_tyiyn: 0, last_purchase_price_tyiyn: 10_000 })
    expect(nb(s.markup ?? '')).toBe('цена продажи 150 с — наценка 50,0 % к последней закупке')
  })
})
