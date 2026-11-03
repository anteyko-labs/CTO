import { describe, expect, it } from 'vitest'
import { OIL_BOOK_DEFAULTS, offlineBookPlan, offlineBookTab, oilBookSettingsOf, shouldAutoReceipt } from './oilBookSettings'

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

describe('книжка для чека без сети при нескольких вкладках кассы', () => {
  /** Вкладки одного устройства: общая запись «книжку взяли» и сообщение «готово» всем. */
  function device() {
    const claimed = new Set<string>()
    const finished = new Set<string>()
    let prints = 0
    const tab = (plan: 'print' | 'notice' | 'none', failPrint = false) =>
      offlineBookTab(plan, {
        claim: async () => (claimed.has('op') ? false : (claimed.add('op'), true)),
        print: async () => {
          if (failPrint) throw new Error('принтер')
          prints += 1
        },
        finished: () => finished.has('op'),
        done: () => finished.add('op'),
      })
    return { tab, prints: () => prints, finished: () => finished.has('op') }
  }

  it('два видимых окна — книжка печатается один раз, второе окно молчит', async () => {
    const d = device()
    const out = await Promise.all([d.tab('print'), d.tab('print')])
    expect(out.sort()).toEqual(['printed', 'skip'])
    expect(d.prints()).toBe(1)
  })

  it('видимая печатает — скрытая уведомления не показывает (или снимает его по «готово»)', async () => {
    const d = device()
    expect(await d.tab('print')).toBe('printed')
    expect(await d.tab('notice')).toBe('skip')
    expect(d.finished()).toBe(true)
  })

  it('видимой вкладки нет — уведомление во всех; печать не удалась — уведомление в печатавшей', async () => {
    const d = device()
    expect(await d.tab('notice')).toBe('notice')
    expect(await d.tab('notice')).toBe('notice')
    const f = device()
    expect(await f.tab('print', true)).toBe('notice')
    expect(f.prints()).toBe(0)
  })
})

describe('товарный чек сам после чека (задача 023)', () => {
  const off = { auto_receipt: false, auto_receipt_all: false }
  const kinds = [
    { serviceWithVehicle: false, byQr: false },
    { serviceWithVehicle: true, byQr: false },
  ]

  it('по умолчанию — нет; у старого снимка поля нет — тоже нет', () => {
    expect(OIL_BOOK_DEFAULTS.auto_receipt_all).toBe(false)
    expect(oilBookSettingsOf({ auto_receipt: true }).auto_receipt_all).toBe(false)
  })

  it('«после любого чека» — печать на вынос и в сервис, с машиной и без', () => {
    for (const k of kinds) {
      expect(shouldAutoReceipt({ auto_receipt: false, auto_receipt_all: true }, k)).toBe(true)
      expect(shouldAutoReceipt({ auto_receipt: true, auto_receipt_all: true }, k)).toBe(true)
    }
  })

  it('выключено — как было: «В сервис» с машиной по auto_receipt, по QR — всегда', () => {
    expect(shouldAutoReceipt(off, kinds[0])).toBe(false)
    expect(shouldAutoReceipt(off, kinds[1])).toBe(false)
    expect(shouldAutoReceipt({ ...off, auto_receipt: true }, kinds[0])).toBe(false)
    expect(shouldAutoReceipt({ ...off, auto_receipt: true }, kinds[1])).toBe(true)
    expect(shouldAutoReceipt(off, { serviceWithVehicle: false, byQr: true })).toBe(true)
  })
})
