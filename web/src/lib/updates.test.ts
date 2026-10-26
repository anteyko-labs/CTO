import { describe, expect, it, vi } from 'vitest'

vi.mock('virtual:pwa-register', () => ({ registerSW: () => async () => undefined }))

const { applyPlan } = await import('./updates')

describe('«Обновить» срабатывает всегда (26.10.2026)', () => {
  it('ждущая версия включилась — её и берём', () => {
    expect(applyPlan({ hasWaiting: true, waitingActivated: true, serverReachable: true })).toBe('activate-waiting')
  })
  it('ждущая не включилась за 3 с — грузим заново с сервера', () => {
    expect(applyPlan({ hasWaiting: true, waitingActivated: false, serverReachable: true })).toBe('fresh-from-server')
  })
  it('ждущей нет — грузим заново с сервера', () => {
    expect(applyPlan({ hasWaiting: false, waitingActivated: null, serverReachable: true })).toBe('fresh-from-server')
  })
  it('сервера нет — только перезагрузка, копию не сбрасываем (касса без сети)', () => {
    expect(applyPlan({ hasWaiting: false, waitingActivated: null, serverReachable: false })).toBe('reload')
    expect(applyPlan({ hasWaiting: true, waitingActivated: false, serverReachable: false })).toBe('reload')
  })
})
