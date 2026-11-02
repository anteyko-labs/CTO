import { describe, expect, it } from 'vitest'
import { ApiError } from './api'
import { barcodesOf, createOrTaken } from './barcodeTaken'
import type { Product } from './types'

const shell = { id: 'p1', name: 'Shell 5W-30 4 л' } as Product
const taken = () => Promise.reject(new ApiError(409, 'conflict', 'штрихкод 460 уже у товара «Shell 5W-30 4 л»'))

describe('createOrTaken (задача 003, Н14)', () => {
  it('заводит товар, если код свободен', async () => {
    expect(await createOrTaken('460', () => Promise.resolve({ id: 'new' }))).toEqual({ taken: false, created: { id: 'new' } })
  })

  it('код в ту же минуту завёл другой — продолжает с его товаром', async () => {
    const r = await createOrTaken('460', taken, (code) => (code === '460' ? Promise.resolve(shell) : Promise.reject(new Error('нет'))))
    expect(r).toEqual({ taken: true, product: shell })
  })

  it('без кода, другая ошибка или товар не нашёлся — прежняя ошибка', async () => {
    const find = () => Promise.resolve(shell)
    await expect(createOrTaken('', taken, find)).rejects.toThrow('уже у товара')
    await expect(createOrTaken('460', () => Promise.reject(new ApiError(422, 'validation', 'укажите цену')), find)).rejects.toThrow('укажите цену')
    await expect(createOrTaken('460', taken, () => Promise.reject(new ApiError(404, 'not_found', 'не найдено')))).rejects.toThrow('уже у товара')
  })
})

describe('barcodesOf', () => {
  it('основной первым, пустые и повторы убраны', () =>
    expect(barcodesOf({ barcode: ' 111 ', more: ['', '222', '111', ' 333'] })).toEqual(['111', '222', '333']))
  it('основной пуст — коды из добавленных', () => expect(barcodesOf({ barcode: '', more: ['222'] })).toEqual(['222']))
  it('ничего — пусто (программа выдаст свой)', () => expect(barcodesOf({ barcode: ' ' })).toEqual([]))
})
