import { describe, expect, it } from 'vitest'
import { OIL_BOOK_DEFAULTS, offlineBookPlan, oilBookSettingsOf } from './oilBookSettings'

describe('книжка для чека без сети (задача 003, хвост Н5)', () => {
  const service = { sale_type: 'service', vehicle_id: 'v1' }

  it('по умолчанию — печатать после отправки; у старого снимка поля нет — тоже да', () => {
    expect(OIL_BOOK_DEFAULTS.offline_book).toBe(true)
    expect(oilBookSettingsOf({ auto_book: true }).offline_book).toBe(true)
  })

  it('«В сервис» с машиной: окно видно — печать, не видно или выключено — кнопка в уведомлении', () => {
    expect(offlineBookPlan(service, { auto_book: true, offline_book: true }, true)).toBe('print')
    expect(offlineBookPlan(service, { auto_book: true, offline_book: true }, false)).toBe('notice')
    expect(offlineBookPlan(service, { auto_book: true, offline_book: false }, true)).toBe('notice')
  })

  it('чек книжки сам не печатается, не «В сервис» или без машины — ничего', () => {
    expect(offlineBookPlan(service, { auto_book: false, offline_book: true }, true)).toBe('none')
    expect(offlineBookPlan({ sale_type: 'takeaway', vehicle_id: 'v1' }, { auto_book: true, offline_book: true }, true)).toBe('none')
    expect(offlineBookPlan({ sale_type: 'service', vehicle_id: null }, { auto_book: true, offline_book: true }, true)).toBe('none')
  })
})
