import { describe, expect, it } from 'vitest'
import { languageProblem, paramLines, recipientText, statusTone, templateNameProblem } from './clientMessages'

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
  })
})
