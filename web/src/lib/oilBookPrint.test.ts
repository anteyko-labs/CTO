import { beforeEach, describe, expect, it, vi } from 'vitest'

const get = vi.fn()
vi.mock('./api', () => ({ get: (path: string) => get(path) }))

describe('адрес бота для печати книжки (SPEC-36 §16)', () => {
  beforeEach(() => {
    get.mockReset()
    vi.resetModules()
  })

  it('спрашивается один раз за сессию', async () => {
    get.mockResolvedValue({ username: 'avtodom_bot' })
    const { botLink } = await import('./oilBookPrint')
    expect(await botLink()).toBe('https://t.me/avtodom_bot')
    expect(await botLink()).toBe('https://t.me/avtodom_bot')
    expect(await botLink()).toBe('https://t.me/avtodom_bot')
    expect(get).toHaveBeenCalledTimes(1)
    expect(get).toHaveBeenCalledWith('/telegram/bot')
  })

  it('бот не подключён — тоже запоминается', async () => {
    get.mockResolvedValue({ username: null })
    const { botLink } = await import('./oilBookPrint')
    expect(await botLink()).toBeNull()
    expect(await botLink()).toBeNull()
    expect(get).toHaveBeenCalledTimes(1)
  })

  it('без ответа сервера не запоминается: следующая печать спросит снова', async () => {
    get.mockRejectedValueOnce(new Error('нет связи')).mockResolvedValue({ username: 'avtodom_bot' })
    const { botLink } = await import('./oilBookPrint')
    expect(await botLink()).toBeNull()
    expect(await botLink()).toBe('https://t.me/avtodom_bot')
    expect(get).toHaveBeenCalledTimes(2)
  })
})
