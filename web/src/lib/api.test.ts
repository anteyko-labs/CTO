import { afterEach, describe, expect, it, vi } from 'vitest'
import { ApiError, api } from './api'

const reply = (body: string, status: number) => vi.fn(async () => new Response(body, { status }))

async function failure(): Promise<ApiError> {
  try {
    await api('GET', '/x')
  } catch (e) {
    if (e instanceof ApiError) return e
    throw e
  }
  throw new Error('ожидалась ошибка')
}

describe('ответы сервера', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('страница ошибки туннеля — «сервер недоступен», а не SyntaxError', async () => {
    vi.stubGlobal('fetch', reply('<html>Bad gateway</html>', 502))
    const e = await failure()
    expect(e.status).toBe(502)
    expect(e.message).toBe('Сервер недоступен, попробуйте позже')
  })

  it('не JSON с прочим кодом — «Ошибка <код>»', async () => {
    vi.stubGlobal('fetch', reply('oops', 500))
    expect((await failure()).message).toBe('Ошибка 500')
  })

  it('ошибка сервера в JSON показывается как есть', async () => {
    vi.stubGlobal('fetch', reply(JSON.stringify({ error: { code: 'x', message: 'Нет товара' } }), 422))
    expect((await failure()).message).toBe('Нет товара')
  })

  it('успешный код с не-JSON телом — понятная ошибка', async () => {
    vi.stubGlobal('fetch', reply('<html></html>', 200))
    const e = await failure()
    expect(e.code).toBe('bad_response')
  })

  it('нет ответа — статус 0', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => {
        throw new TypeError('Failed to fetch')
      }),
    )
    expect((await failure()).status).toBe(0)
  })
})
