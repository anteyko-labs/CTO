import { describe, expect, it } from 'vitest'
import { modelsOf } from './carDict'

const makes = [
  { id: '1', name: 'Toyota', models: [{ id: 'a', name: 'Camry' }] },
  { id: '2', name: 'Lada', models: [] },
]

describe('справочник машин', () => {
  it('модели марки — без учёта регистра; новая марка — без подсказок', () => {
    expect(modelsOf(makes, ' toyota ').map((m) => m.name)).toEqual(['Camry'])
    expect(modelsOf(makes, 'Tesla')).toEqual([])
    expect(modelsOf(makes, '')).toEqual([])
  })
})
