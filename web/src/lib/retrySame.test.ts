import { describe, expect, it } from 'vitest'
import { ApiError } from './api'
import { retrySame } from './retrySame'

const noWait = () => Promise.resolve()

describe('retrySame', () => {
  it('нет ответа сервера — тот же запрос ещё раз, с тем же телом', async () => {
    const bodies: string[] = []
    let n = 0
    const body = { op_id: 'op-1', counted_qty: 1 }
    const r = await retrySame(
      async () => {
        bodies.push(JSON.stringify(body))
        if (n++ < 2) throw new ApiError(0, 'network', 'Нет связи с сервером')
        return 'ok'
      },
      2,
      0,
      noWait,
    )
    expect(r).toBe('ok')
    expect(bodies).toEqual(Array(3).fill(JSON.stringify(body)))
  })

  it('повторов не больше заданного', async () => {
    let n = 0
    await expect(
      retrySame(
        async () => {
          n++
          throw new ApiError(504, 'error', 'Сервер недоступен')
        },
        2,
        0,
        noWait,
      ),
    ).rejects.toThrow('Сервер недоступен')
    expect(n).toBe(3)
  })

  it('сервер ответил отказом — не повторяет', async () => {
    let n = 0
    await expect(
      retrySame(
        async () => {
          n++
          throw new ApiError(409, 'count_changed', 'сейчас 3 шт')
        },
        2,
        0,
        noWait,
      ),
    ).rejects.toThrow('сейчас 3 шт')
    expect(n).toBe(1)
  })
})
