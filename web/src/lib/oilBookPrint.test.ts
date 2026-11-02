import { beforeEach, describe, expect, it, vi } from 'vitest'

const get = vi.fn()
vi.mock('./api', () => ({ get: (path: string) => get(path) }))
const { labelLines } = await import('./oilBookPrint')

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

describe('наклейки книжки 58×40 (задача 009)', () => {
  it('что залито — по видам замены, следующая замена — общая для вида или по строке', () => {
    const { filled, next } = labelLines([
      { title: 'Mobil 1 5W-30 · 4,5 л', group: 'Масло двигателя', next: '155 300 км' },
      { title: 'W712 · 1 шт', group: 'Масло двигателя', next: '155 300 км' },
      { title: 'C 25 114 · 1 шт', group: 'Воздушный фильтр', next: '160 300 км' },
    ])
    expect(filled).toEqual(['Масло двигателя: Mobil 1 5W-30 · 4,5 л, W712 · 1 шт', 'Воздушный фильтр: C 25 114 · 1 шт'])
    expect(next).toEqual(['Масло двигателя — 155 300 км', 'Воздушный фильтр — 160 300 км'])
  })

  it('разные сроки внутри вида — каждой строкой', () => {
    const { next } = labelLines([
      { title: 'Масло', group: 'Коробка', next: '60 000 км' },
      { title: 'Фильтр', group: 'Коробка', next: '' },
    ])
    expect(next).toEqual(['Масло — 60 000 км'])
  })
})
