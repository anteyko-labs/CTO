import { describe, expect, it } from 'vitest'
import { BLANK, debtHistory, numberInWords, renderTemplate, somInWords, type Reconciliation, type ReconRow } from './debtDocs'

describe('сумма прописью', () => {
  it('сомы словами, тыйыны цифрами (SPEC-10)', () => {
    expect(somInWords(120_050)).toBe('Одна тысяча двести сом 50 тыйын')
    expect(somInWords(700_000)).toBe('Семь тысяч сом 00 тыйын')
    expect(somInWords(0)).toBe('Ноль сом 00 тыйын')
  })

  it('род и падеж разрядов', () => {
    expect(numberInWords(2_000)).toBe('две тысячи')
    expect(numberInWords(5_000)).toBe('пять тысяч')
    expect(numberInWords(11_000)).toBe('одиннадцать тысяч')
    expect(numberInWords(21_001)).toBe('двадцать одна тысяча один')
    expect(numberInWords(1_000_000)).toBe('один миллион')
    expect(numberInWords(2_342_115)).toBe('два миллиона триста сорок две тысячи сто пятнадцать')
  })
})

describe('шаблон', () => {
  it('подставляет значения, пустое печатает линией, текст экранирует', () => {
    const out = renderTemplate('Я, {{клиент}}, ПИН {{инн}} <b>', { клиент: 'Эрлан', инн: '' })
    expect(out).toBe(`Я, Эрлан, ПИН ${BLANK} &lt;b&gt;`)
  })

  it('таблицу товаров вставляет как HTML', () => {
    expect(renderTemplate('{{товары}}', {}, { товары: '<table></table>' })).toBe('<table></table>')
  })
})

const row = (doc_id: string, debit: number, credit = 0, doc_type = 'sale'): ReconRow => ({
  date: '2026-10-05',
  document: doc_type === 'sale' ? `Отгрузка товара, чек № ${doc_id}` : 'Оплата наличными',
  doc_type,
  doc_id,
  debit_tyiyn: debit,
  credit_tyiyn: credit,
  comment: '',
})

const act = (rows: ReconRow[]): Reconciliation => ({
  party_id: 'p',
  name: 'Эрлан',
  kind: 'person',
  role: 'customer',
  inn: '',
  phone: '',
  from: '2026-10-01',
  to: '2026-10-09',
  opening_tyiyn: 0,
  rows,
  debit_total_tyiyn: 0,
  credit_total_tyiyn: 0,
  closing_tyiyn: 0,
})

describe('история долга в документе', () => {
  it('взял 400, потом ещё 500: прежний долг 400, общий 900', () => {
    const h = debtHistory(act([row('s1', 40_000), row('s2', 50_000)]), 's2')
    expect(h?.rows.map((r) => r.doc_id)).toEqual(['s1'])
    expect(h?.before).toBe(40_000)
    expect(h?.after).toBe(90_000)
  })

  it('прежний долг погашен целиком — истории нет', () => {
    const h = debtHistory(act([row('s1', 40_000), row('r1', 0, 40_000, 'repayment'), row('s2', 50_000)]), 's2')
    expect(h?.rows).toEqual([])
    expect(h?.before).toBe(0)
  })

  it('частичная оплата видна в истории', () => {
    const h = debtHistory(act([row('s1', 40_000), row('r1', 0, 10_000, 'repayment'), row('s2', 50_000)]), 's2')
    expect(h?.rows).toHaveLength(2)
    expect(h?.before).toBe(30_000)
    expect(h?.after).toBe(80_000)
  })

  it('документ позже: баланс на момент чека, а не сегодняшний', () => {
    const h = debtHistory(act([row('s1', 40_000), row('s2', 50_000), row('r1', 0, 90_000, 'repayment')]), 's2')
    expect(h?.after).toBe(90_000)
  })
})
