import { describe, expect, it } from 'vitest'
import { carName, modelsOf } from './carDict'

const makes = [
  { id: '1', name: 'Toyota', models: [{ id: 'a', name: 'Camry' }] },
  { id: '3', name: 'Lexus', models: [{ id: 'b', name: 'RX 350' }] },
  { id: '2', name: 'Lada', models: [] },
]

describe('справочник машин', () => {
  it('модели марки — без учёта регистра; новая марка — без подсказок', () => {
    expect(modelsOf(makes, ' toyota ').map((m) => m.name)).toEqual(['Camry'])
    expect(modelsOf(makes, 'Tesla')).toEqual([])
    expect(modelsOf(makes, '')).toEqual([])
  })
})

describe('написание марки и модели', () => {
  it('из справочника — его написание, без учёта регистра и пробелов', () => {
    const lexus = modelsOf(makes, 'lexus').map((m) => m.name)
    expect(carName('rx 350', lexus)).toBe('RX 350')
    expect(carName(' RX350 ', lexus)).toBe('RX 350')
    expect(carName('toyota', makes.map((m) => m.name))).toBe('Toyota')
  })

  it('нового слова в справочнике нет: строчные — с заглавной, коды — заглавными', () => {
    expect(carName('land  cruiser', [])).toBe('Land Cruiser')
    expect(carName('rav4', [])).toBe('RAV4')
    expect(carName('cx-5', [])).toBe('CX-5')
    expect(carName('bmw', [])).toBe('BMW')
    expect(carName('Prius alpha', [])).toBe('Prius alpha')
    expect(carName('  ', [])).toBe('')
  })
})
