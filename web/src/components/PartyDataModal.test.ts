import { describe, expect, it } from 'vitest'
import { debtDataMissing } from './PartyDataModal'

const person = { kind: 'person' as const, name: 'Асанов Бакыт', first_name: 'Бакыт', last_name: 'Асанов', phone: '+996700000001', inn: '21234567890123' }

describe('debtDataMissing (задача 048)', () => {
  it('физлицо с полными данными — ничего', () => {
    expect(debtDataMissing(person)).toEqual([])
  })
  it('старый долг из тетради: только имя', () => {
    expect(debtDataMissing({ ...person, last_name: '', phone: '', inn: '' })).toEqual(['фамилия', 'телефон', 'ПИН'])
  })
  it('ПИН — ровно 14 цифр', () => {
    expect(debtDataMissing({ ...person, inn: '1234567890' })).toEqual(['ПИН'])
  })
  it('фирма — название, телефон, ИНН', () => {
    expect(debtDataMissing({ kind: 'company', name: 'ОсОО Ак', phone: '', inn: '01234567890123' })).toEqual(['телефон'])
    expect(debtDataMissing({ kind: 'company', name: ' ', phone: '1', inn: '12' })).toEqual(['название', 'ИНН'])
  })
})
