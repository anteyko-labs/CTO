import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiError } from './api'
import { createEntrySaver, entryEmpty, pickDraft, readLocalDraft, resolveLines, type EntryDraft, type EntryDraftBody, type EntryDraftLine } from './entryDraft'
import type { Product } from './types'

const body = (comment = 'привёз'): EntryDraftBody => ({ supplierId: '', supplierDoc: '', comment, payMode: 'paid', partText: '', paidFrom: '', lines: [] })
const line = (id: string): EntryDraftLine => ({ product_id: id, name: `Товар ${id}`, count: '2', price: '10', markup: '', salePrice: '20' })
const draft = (seq: number): EntryDraft => ({ kind: 'stock_in', op_id: 'op', seq, body: body() })

const store = new Map<string, string>()
beforeEach(() => {
  store.clear()
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, v),
    removeItem: (k: string) => void store.delete(k),
  })
})

const saver = (send: (d: EntryDraft) => Promise<unknown>) => {
  const onStatus = vi.fn()
  const onPosted = vi.fn()
  const s = createEntrySaver({ storageKey: (k) => `k.${k}`, onStatus, onPosted, send, delayMs: 10_000, retryMs: 10_000 })
  return { s, onStatus, onPosted }
}

describe('форма и черновик', () => {
  it('пустая форма не хранится, форма с комментарием хранится', () => {
    expect(entryEmpty(body(''))).toBe(true)
    expect(entryEmpty(body())).toBe(false)
    expect(entryEmpty({ ...body(''), lines: [line('a')] })).toBe(false)
  })

  it('копия устройства — только если новее сервера', () => {
    expect(pickDraft(draft(5), draft(6))?.seq).toBe(6)
    expect(pickDraft(draft(5), draft(5))?.seq).toBe(5)
    expect(pickDraft(draft(5), draft(4))?.seq).toBe(5)
    expect(pickDraft(null, draft(1))?.seq).toBe(1)
    expect(pickDraft(null, null)).toBeNull()
  })

  it('товар в архиве и удалённый убираются с названием, остальные — с текущей карточкой', async () => {
    const r = await resolveLines([line('a'), line('b'), line('c')], (id) => {
      if (id === 'c') return Promise.reject(new ApiError(404, 'not_found', 'нет'))
      return Promise.resolve({ id, archived: id === 'b', sale_price_tyiyn: 77 } as Product)
    })
    expect(r.lines.map((l) => l.product.id)).toEqual(['a'])
    expect(r.lines[0].product.sale_price_tyiyn).toBe(77)
    expect(r.lines[0].line.count).toBe('2')
    expect(r.removed).toEqual(['Товар b', 'Товар c'])
  })

  it('сбой связи при чтении товара строку не убирает', async () => {
    await expect(resolveLines([line('a')], () => Promise.reject(new ApiError(0, 'network', 'нет')))).rejects.toThrow()
  })
})

describe('запись', () => {
  it('копия — сразу, после ответа сервера убирается; новая запись новее', async () => {
    const sent: EntryDraft[] = []
    const { s, onStatus } = saver((d) => {
      sent.push(d)
      return Promise.resolve()
    })
    s.change('stock_in', 'op', body('раз'))
    expect(readLocalDraft('k.stock_in')?.body.comment).toBe('раз')
    s.change('stock_in', 'op', body('два'))
    await s.flush()
    expect(sent.map((d) => d.body.comment)).toEqual(['два'])
    expect(readLocalDraft('k.stock_in')).toBeNull()
    expect(onStatus).toHaveBeenLastCalledWith('saved')
    s.change('stock_in', 'op', body('три'))
    await s.flush()
    expect(sent[1].seq).toBeGreaterThan(sent[0].seq)
    s.finish('stock_in')
  })

  it('без связи копия остаётся и запись уходит при связи', async () => {
    let up = false
    const sent: string[] = []
    const { s, onStatus } = saver((d) => {
      if (!up) return Promise.reject(new ApiError(0, 'network', 'нет'))
      sent.push(d.body.comment)
      return Promise.resolve()
    })
    s.change('stock_in', 'op', body())
    await s.flush()
    expect(onStatus).toHaveBeenLastCalledWith('offline')
    expect(readLocalDraft('k.stock_in')).not.toBeNull()
    up = true
    await s.flush()
    expect(sent).toEqual(['привёз'])
    expect(readLocalDraft('k.stock_in')).toBeNull()
    s.finish('stock_in')
  })

  it('409: документ уже проведён — копия убирается, экран узнаёт', async () => {
    const { s, onPosted } = saver(() => Promise.reject(new ApiError(409, 'conflict', 'проведён')))
    s.change('stock_in', 'op', body())
    await s.flush()
    expect(onPosted).toHaveBeenCalledOnce()
    expect(readLocalDraft('k.stock_in')).toBeNull()
    s.finish('stock_in')
  })

  it('закрытие вкладки отправляет ожидающее; после проведения записей нет', async () => {
    const closing = vi.fn()
    const send = vi.fn(() => Promise.resolve())
    const s = createEntrySaver({ storageKey: (k) => `k.${k}`, onStatus: vi.fn(), onPosted: vi.fn(), send, sendClosing: closing, delayMs: 10_000 })
    s.change('stock_in', 'op', body())
    s.closing()
    expect(closing).toHaveBeenCalledOnce()
    expect(readLocalDraft('k.stock_in')).not.toBeNull()
    s.finish('stock_in')
    expect(readLocalDraft('k.stock_in')).toBeNull()
    s.change('stock_in', 'op', body())
    await s.flush()
    expect(send).not.toHaveBeenCalled()
  })

  it('сброс: ожидающая запись не уходит, сервер получает удаление', async () => {
    const send = vi.fn(() => Promise.resolve())
    const remove = vi.fn(() => Promise.resolve())
    const { s } = saver(send)
    s.change('stock_in', 'op', body())
    await s.discard('stock_in', remove)
    expect(send).not.toHaveBeenCalled()
    expect(remove).toHaveBeenCalledOnce()
    expect(readLocalDraft('k.stock_in')).toBeNull()
    s.finish('stock_in')
  })
})
