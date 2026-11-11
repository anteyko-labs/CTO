import { describe, expect, it } from 'vitest'
import { acceptanceActHtml } from './acceptanceAct'
import type { Acceptance, AcceptanceLine } from './acceptances'

const seller = { name: 'ИП Точка', inn: '123', address: 'Бишкек', phone: '', director: '', bank: '', city: 'Бишкек' }
const line = (o: Partial<AcceptanceLine>): AcceptanceLine =>
  ({ line_no: 1, product_id: 'p', name: 'Масло 5W30', article: 'A1', barcodes: [], unit: 'piece', container_ml: null, invoice_qty: 4, line_cost_tyiyn: 400_000, sale_price_tyiyn: null, accepted_qty: 3, accepted_name: null, accepted_container_ml: null, reason: 'damaged', comment: '', ...o }) as AcceptanceLine
const acc = (o: Partial<Acceptance> = {}): Acceptance =>
  ({ id: 'a', status: 'accepted', supplier_id: 's', supplier_name: 'ОсОО Поставщик', supplier_doc: '77', comment: '', sent_by_name: 'Владелец', sent_at: '2026-11-11T04:00:00Z', from_bot: false, draft_id: null, photo_type: null, lines_count: 1, total_tyiyn: 400_000, lines: [line({})], extras: [], closed_at: '2026-11-11T05:00:00Z', closed_by_name: 'Кассир', employee_name: 'Айбек', reason: null, accept_comment: '', receipt_id: 'r', receipt_number: 12, receipt_state: null, corrected_id: null, corrected_number: null, has_signature: false, ...o }) as Acceptance

describe('акт приёмки', () => {
  it('шапка: номер, дата, поставщик, накладная', () => {
    const h = acceptanceActHtml(acc(), seller, null)
    expect(h).toContain('Акт приёмки товара № 12 от 11.11.2026')
    expect(h).toContain('ОсОО Поставщик')
    expect(h).toContain('накладная № 77')
  })
  it('строка с расхождением и причиной', () => {
    const h = acceptanceActHtml(acc(), seller, null)
    expect(h).toContain('−1 шт')
    expect(h).toContain('брак или повреждено')
  })
  it('сверх накладной — отдельным блоком', () => {
    expect(acceptanceActHtml(acc({ extras: [{ name: 'Фильтр', barcode: '1', qty: 2 }] }), seller, null)).toContain('Фильтр — 2 шт')
  })
  it('графы «Сдал» и «Принял» с датой', () => {
    const h = acceptanceActHtml(acc(), seller, null)
    expect(h).toContain('Сдал (поставщик / водитель)')
    expect(h).toContain('Принял: Айбек')
    expect(h).toContain('Дата: 11.11.2026')
  })
  it('подпись кассира картинкой, без неё — линия', () => {
    expect(acceptanceActHtml(acc(), seller, 'data:image/png;base64,AAAA')).toContain('<img src="data:image/png;base64,AAAA"')
    expect(acceptanceActHtml(acc(), seller, null)).not.toContain('<img')
  })
  it('цены и суммы — только когда сервер их отдал', () => {
    expect(acceptanceActHtml(acc(), seller, null)).toContain('Итого по накладной')
    const h = acceptanceActHtml(acc({ total_tyiyn: null, lines: [line({ line_cost_tyiyn: null })] }), seller, null)
    expect(h).not.toContain('Сумма')
    expect(h).not.toContain('Итого')
  })
})
