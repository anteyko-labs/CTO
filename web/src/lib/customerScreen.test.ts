import { describe, expect, it } from 'vitest'
import { PAID_SHOW_MS, qrToScreen, screenStep, screenTick, toScreen, type ScreenState } from './customerScreen'

const idle: ScreenState = { kind: 'idle' }

describe('экран покупателя (задача 008)', () => {
  it('QR → «Оплачено» → заставка через 5 с', () => {
    const qr = screenStep(idle, { type: 'qr', amount_tyiyn: 150_000, qr_base64: 'AAA' }, 0)
    expect(qr).toEqual({ kind: 'qr', amount_tyiyn: 150_000, qr_base64: 'AAA' })
    const paid = screenStep(qr, { type: 'paid', amount_tyiyn: 150_000 }, 1000)
    expect(paid.kind).toBe('paid')
    // Касса уже очистила чек — «Оплачено» досматривается.
    expect(screenStep(paid, { type: 'idle' }, 2000).kind).toBe('paid')
    expect(screenTick(paid, 1000 + PAID_SHOW_MS - 1).kind).toBe('paid')
    expect(screenTick(paid, 1000 + PAID_SHOW_MS)).toEqual(idle)
  })

  it('подпись не перебивается QR и заставкой, снимается своей отменой', () => {
    const sign = screenStep(idle, { type: 'sign', id: 'a', title: 'Подпишите', lines: [] }, 0)
    expect(screenStep(sign, { type: 'idle' }, 1).kind).toBe('sign')
    expect(screenStep(sign, { type: 'qr', amount_tyiyn: 1, qr_base64: 'A' }, 1).kind).toBe('sign')
    expect(screenStep(sign, { type: 'sign-cancel', id: 'b' }, 1).kind).toBe('sign')
    expect(screenStep(sign, { type: 'sign-cancel', id: 'a' }, 1)).toEqual(idle)
  })

  it('QR кассы: ждущий с картинкой, оплаченный — сумма банка, остальное — заставка', () => {
    expect(qrToScreen({ id: '1', status: 'waiting', amount_tyiyn: 100, qr_base64: 'Q' })).toEqual({ type: 'qr', amount_tyiyn: 100, qr_base64: 'Q' })
    expect(qrToScreen({ id: '1', status: 'waiting', amount_tyiyn: 100, qr_base64: null })).toEqual({ type: 'idle' })
    expect(qrToScreen({ id: '1', status: 'paid', amount_tyiyn: 100, paid_tyiyn: 90 })).toEqual({ type: 'paid', amount_tyiyn: 90 })
    expect(qrToScreen({ id: '1', status: 'unpaid', amount_tyiyn: 100 })).toEqual({ type: 'idle' })
    expect(qrToScreen(null)).toEqual({ type: 'idle' })
  })
})

describe('дубль кассы на экране покупателя', () => {
  const line = { name: 'Mobil 1 5W-30', qty: '1 шт', amount_tyiyn: 350_000 }
  it('без QR — чек кассы, с QR — QR, пустой чек — заставка', () => {
    expect(toScreen(null, [line], 350_000)).toEqual({ type: 'cart', lines: [line], total_tyiyn: 350_000 })
    expect(toScreen({ id: '1', status: 'waiting', amount_tyiyn: 350_000, qr_base64: 'Q' }, [line], 350_000).type).toBe('qr')
    expect(toScreen(null, [], 0)).toEqual({ type: 'idle' })
  })

  it('«Оплачено» не перебивается следующим чеком, подпись — тоже', () => {
    const paid = screenStep(idle, { type: 'paid', amount_tyiyn: 1 }, 0)
    expect(screenStep(paid, { type: 'cart', lines: [line], total_tyiyn: 1 }, 1).kind).toBe('paid')
    expect(screenStep(paid, { type: 'cart', lines: [line], total_tyiyn: 1 }, PAID_SHOW_MS).kind).toBe('cart')
    const sign = screenStep(idle, { type: 'sign', id: 'a', title: '', lines: [] }, 0)
    expect(screenStep(sign, { type: 'cart', lines: [line], total_tyiyn: 1 }, 1).kind).toBe('sign')
  })
})
