import { describe, expect, it } from 'vitest'
import { docsGoToStation, rememberDocRoute } from './docPrint'

const store = () => {
  const m = new Map<string, string>()
  return {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, v),
    removeItem: (k: string) => void m.delete(k),
  }
}

describe('маршрут документов A4 (задача 034)', () => {
  it('по умолчанию устройство печатает у себя', () => {
    expect(docsGoToStation(store())).toBe(false)
  })

  it('ярлык кассы включает станцию, и она остаётся включённой без параметра', () => {
    const s = store()
    rememberDocRoute('?docs=station', s)
    expect(docsGoToStation(s)).toBe(true)
    rememberDocRoute('', s)
    expect(docsGoToStation(s)).toBe(true)
  })

  it('?docs=local выключает станцию', () => {
    const s = store()
    rememberDocRoute('?docs=station', s)
    rememberDocRoute('?docs=local', s)
    expect(docsGoToStation(s)).toBe(false)
  })
})
