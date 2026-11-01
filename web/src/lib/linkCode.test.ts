import { describe, expect, it, vi } from 'vitest'
import { ApiError } from './api'
import { linkBody, linkCode, type LinkDeps } from './linkCode'
import type { Product } from './types'

const prod = (id: string, name = id) => ({ id, name }) as Product

function deps(o: Partial<LinkDeps>): LinkDeps {
  return {
    send: vi.fn(async () => ({})),
    product: vi.fn(async (id: string) => prod(id)),
    byBarcode: vi.fn(async () => prod('other')),
    ...o,
  }
}

describe('привязка кода на кассе (задача 005 Н3)', () => {
  it('тело — с op_id; код меры — только по согласию', () => {
    const b = linkBody('4006381333931')
    expect(typeof b.op_id).toBe('string')
    expect(b).toMatchObject({ code: '4006381333931' })
    expect(b).not.toHaveProperty('measure_ok')
    expect(linkBody('2760275010002', true)).toMatchObject({ measure_ok: true })
    expect(linkBody(undefined)).not.toHaveProperty('code')
  })

  it('привязан — товар с сервера', async () => {
    const d = deps({})
    expect(await linkCode('a', '123456', false, d)).toEqual({ kind: 'linked', product: prod('a') })
    expect(d.send).toHaveBeenCalledWith('a', expect.objectContaining({ code: '123456' }))
  })

  it('409: код уже у этого товара — успех, у другого — предложить его', async () => {
    const taken = () => Promise.reject(new ApiError(409, 'conflict', 'штрихкод 123456 уже у товара «Y»'))
    expect(await linkCode('a', '123456', false, deps({ send: taken, byBarcode: async () => prod('a') }))).toEqual({ kind: 'linked', product: prod('a') })
    expect(await linkCode('a', '123456', false, deps({ send: taken, byBarcode: async () => prod('y', 'Y') }))).toEqual({ kind: 'taken', product: prod('y', 'Y') })
  })

  it('код литра из Умага — вопрос; другая ошибка — как есть', async () => {
    const measure = () => Promise.reject(new ApiError(409, 'umag_measure_code', 'Код … Точно привязать?'))
    expect(await linkCode('a', '2760275010002', false, deps({ send: measure }))).toEqual({ kind: 'measure', message: 'Код … Точно привязать?' })
    const denied = () => Promise.reject(new ApiError(403, 'forbidden', 'Нет права: Привязывать коды на кассе'))
    await expect(linkCode('a', '123456', false, deps({ send: denied }))).rejects.toThrow('Нет права')
  })
})
