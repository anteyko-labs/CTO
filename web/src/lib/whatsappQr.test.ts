import { describe, expect, it } from 'vitest'
import { codeText, pairPhoneProblem, polling, STATE_LABELS } from './whatsappQr'

describe('WhatsApp по QR-коду (задача 052)', () => {
  it('состояния и опрос', () => {
    expect(polling('qr')).toBe(true)
    expect(polling('code')).toBe(true)
    expect(polling('connected')).toBe(false)
    expect(STATE_LABELS.logged_out).toContain('подключите заново')
  })
  it('код из 8 знаков и номер', () => {
    expect(codeText('abcd1234')).toBe('ABCD-1234')
    expect(codeText('ABCD-1234')).toBe('ABCD-1234')
    expect(pairPhoneProblem('0555 12 34 56')).toBeNull()
    expect(pairPhoneProblem('')).toContain('введите')
    expect(pairPhoneProblem('123')).not.toBeNull()
  })
})
