import { describe, expect, it } from 'vitest'
import {
  DEBT_DUE_MARKS,
  PAYMENT_MARKS,
  SAMPLE_VALUES,
  languageProblem,
  paramLines,
  recipientText,
  sampleText,
  statusTone,
  templateNameProblem,
  textProblem,
  unknownMarks,
} from './clientMessages'

describe('Сообщения клиентам (SPEC-32)', () => {
  it('имя и язык шаблона Meta', () => {
    expect(templateNameProblem('avtodom_oil_change')).toBeNull()
    expect(templateNameProblem('')).toBeNull()
    expect(templateNameProblem('Замена масла')).not.toBeNull()
    expect(templateNameProblem('oil-change')).not.toBeNull()
    expect(languageProblem('ru')).toBeNull()
    expect(languageProblem('en_US')).toBeNull()
    expect(languageProblem('r')).not.toBeNull()
  })

  it('переменные по порядку и журнал', () => {
    expect(paramLines(['имя клиента', 'машина'])).toEqual(['{{1}} — имя клиента', '{{2}} — машина'])
    expect(recipientText({ channel: 'whatsapp', recipient: '+996555123456' })).toBe('+996555123456')
    expect(recipientText({ channel: 'telegram', recipient: '42' })).toBe('чат 42')
    expect(recipientText({ channel: 'none', recipient: '' })).toBe('—')
    expect(statusTone('sent')).toBe('green')
    expect(statusTone('error')).toBe('rose')
    expect(recipientText({ channel: 'cabinet', recipient: '' })).toBe('—')
  })

  it('тексты должникам: подстановки и образец (задача 002, П7, П9)', () => {
    expect(unknownMarks('{имя}, {телефон} {имя} {сумма}', PAYMENT_MARKS)).toEqual(['телефон'])
    expect(textProblem('Здравствуйте, {имя}! Получили оплату {сумма}.', PAYMENT_MARKS)).toBeNull()
    expect(textProblem('Привет, {телефон}', PAYMENT_MARKS)).toContain('{телефон}')
    expect(textProblem('  ', PAYMENT_MARKS)).not.toBeNull()
    expect(textProblem('', PAYMENT_MARKS, true)).toBeNull()
    expect(textProblem('Срок {когда}', DEBT_DUE_MARKS)).toBeNull()
    expect(sampleText('Здравствуйте, {имя}! Получили оплату {сумма}. {x}', SAMPLE_VALUES)).toBe(
      'Здравствуйте, Азиз! Получили оплату 5 000 сом. {x}',
    )
  })
})
