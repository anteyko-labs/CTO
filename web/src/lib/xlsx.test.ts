import { describe, expect, it } from 'vitest'
import { columnIndex, containerFromName, somToTyiyn } from './xlsx'

describe('Excel из старой программы', () => {
  it('столбцы', () => {
    expect(columnIndex('A1')).toBe(0)
    expect(columnIndex('C12')).toBe(2)
    expect(columnIndex('AA3')).toBe(26)
  })
  it('суммы в тыйынах без дробей', () => {
    expect(somToTyiyn('150.813')).toBe(15081)
    expect(somToTyiyn('150.815')).toBe(15082)
    expect(somToTyiyn('550')).toBe(55000)
    expect(somToTyiyn('1 200,5')).toBe(120050)
    expect(somToTyiyn('')).toBe(0)
    expect(somToTyiyn('abc')).toBe(0)
  })
  it('объём тары из названия', () => {
    expect(containerFromName('AZMOL 5W40 LEADER PLUS 4Л')).toBe(4000)
    expect(containerFromName('AZMOL DISEL 10W40 200L')).toBe(200000)
    expect(containerFromName('ЛУКОИЛ 10W40 205Л')).toBe(205000)
    expect(containerFromName('MOBIL 0,946L')).toBe(946)
    expect(containerFromName('WD 40')).toBeNull()
  })
})
