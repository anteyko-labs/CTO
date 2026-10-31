import { describe, expect, it } from 'vitest'
import { actDocLink, actLines, actPeriod, actVerdict, BLANK, debtHistory, numberInWords, renderTemplate, somInWords, type Reconciliation, type ReconRow } from './debtDocs'

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

describe('акт сверки на экране (задача 003 Н9)', () => {
  it('быстрые периоды: этот месяц, прошлый месяц, с начала года, всё время', () => {
    expect(actPeriod('month', '2026-10-31')).toEqual({ from: '2026-10-01', to: '2026-10-31' })
    expect(actPeriod('last_month', '2026-10-31')).toEqual({ from: '2026-09-01', to: '2026-09-30' })
    // Январь: прошлый месяц — декабрь прошлого года.
    expect(actPeriod('last_month', '2026-01-15')).toEqual({ from: '2025-12-01', to: '2025-12-31' })
    expect(actPeriod('last_month', '2028-03-10')).toEqual({ from: '2028-02-01', to: '2028-02-29' })
    expect(actPeriod('year', '2026-10-31')).toEqual({ from: '2026-01-01', to: '2026-10-31' })
    expect(actPeriod('all', '2026-10-31')).toEqual({ from: '', to: '2026-10-31' })
  })

  it('строки как в печатном акте: сальдо, документы, обороты, сальдо; долг — в дебете, аванс — в кредите', () => {
    const a: Reconciliation = {
      ...act([row('s1', 40_000), row('r1', 0, 100_000, 'repayment')]),
      opening_tyiyn: 20_000,
      debit_total_tyiyn: 40_000,
      credit_total_tyiyn: 100_000,
      closing_tyiyn: -40_000,
    }
    const lines = actLines(a)
    expect(lines.map((l) => l.kind)).toEqual(['opening', 'row', 'row', 'totals', 'closing'])
    expect(lines[0]).toMatchObject({ document: 'Сальдо на 01.10.2026', debit_tyiyn: 20_000, credit_tyiyn: 0 })
    expect(lines[3]).toMatchObject({ debit_tyiyn: 40_000, credit_tyiyn: 100_000 })
    expect(lines[4]).toMatchObject({ document: 'Сальдо на 09.10.2026', debit_tyiyn: 0, credit_tyiyn: 40_000 })
    // Сальдо сходится: на начало + дебет − кредит = на конец.
    expect(a.opening_tyiyn + a.debit_total_tyiyn - a.credit_total_tyiyn).toBe(a.closing_tyiyn)
    expect(lines.every((l) => Number.isInteger(l.debit_tyiyn) && Number.isInteger(l.credit_tyiyn))).toBe(true)
  })

  it('ссылки — на чек и приход; вывод — кто кому должен', () => {
    expect(actDocLink({ doc_type: 'sale', doc_id: 's1' })).toBe('/sales/s1')
    expect(actDocLink({ doc_type: 'receipt', doc_id: 'r1' })).toBe('/receipts/r1')
    expect(actDocLink({ doc_type: 'repayment', doc_id: 'p1' })).toBeNull()
    expect(actDocLink({ doc_type: 'sale', doc_id: null })).toBeNull()
    expect(actVerdict({ ...act([]), closing_tyiyn: 0 }, 'Автодом')).toBe('На 09.10.2026 долга между сторонами нет.')
    expect(actVerdict({ ...act([]), closing_tyiyn: 90_000 }, 'Автодом')).toMatch(/^На 09\.10\.2026 Эрлан должен Автодом 900/)
    expect(actVerdict({ ...act([]), closing_tyiyn: -5_000 }, '')).toMatch(/^На 09\.10\.2026 точка должна Эрлан 50/)
  })
})
