import { describe, expect, it } from 'vitest'
import { canAutoApply } from './updateRules'

const at = (patch: Partial<Parameters<typeof canAutoApply>[0]>) => canAutoApply({ path: '/', cartEmpty: true, modalOpen: false, visible: true, ...patch })

describe('новая версия применяется сама', () => {
  it('на кассе с пустым чеком и без окна — да', () => {
    expect(at({})).toBe(true)
  })

  it('чек не пуст, открыто окно или вкладка скрыта — нет', () => {
    expect(at({ cartEmpty: false })).toBe(false)
    expect(at({ modalOpen: true })).toBe(false)
    expect(at({ visible: false })).toBe(false)
  })

  it('на других экранах — только плашка', () => {
    for (const path of ['/receipts/new', '/revision', '/intake', '/shift', '/sales/1', '/products']) {
      expect(at({ path })).toBe(false)
    }
  })
})
