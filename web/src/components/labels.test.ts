import { beforeAll, describe, expect, it, vi } from 'vitest'
import type { LabelSettings, Product } from '../lib/types'

// В тестах нет DOM: штрихкод — заглушка, проверяется только текст этикетки.
vi.mock('jsbarcode', () => ({ default: () => undefined }))
beforeAll(() => {
  const el = { setAttribute: () => undefined, removeAttribute: () => undefined, outerHTML: '<svg data-code></svg>' }
  vi.stubGlobal('document', { createElementNS: () => el })
})

const product = {
  id: 'p1',
  name: 'Фильтр масляный',
  brand: 'Mann',
  article: null,
  unit: 'piece',
  container_ml: null,
  attrs: {},
  barcodes: ['2000000000015'],
  sale_price_tyiyn: 50_000,
} as unknown as Product

const big: LabelSettings = { width_mm: 40, height_mm: 30, show_name: true, show_price: true, show_article: false, show_brand: true, show_spec: true }
const small: LabelSettings = { width_mm: 30, height_mm: 20, show_name: false, show_price: true, show_article: false, show_brand: false, show_spec: false }

describe('этикетки большая и маленькая (задача 014)', () => {
  it('большая 40×30 — название и «Цена:»', async () => {
    const { labelsHtml } = await import('./labels')
    const html = labelsHtml([{ product, copies: 1 }], big)
    expect(html).toContain('Фильтр масляный')
    expect(html).toContain('Цена: ')
    expect(html).toContain('size: 40mm 30mm')
  })
  it('маленькая 30×20 — штрихкод и «Цена:», без названия', async () => {
    const { labelsHtml } = await import('./labels')
    const html = labelsHtml([{ product, copies: 2 }], small)
    expect(html).toContain('size: 30mm 20mm')
    expect(html).toContain('data-code')
    expect(html).toContain('class="price big">Цена: ')
    expect(html).not.toContain('Фильтр масляный')
    expect(html).not.toContain('Mann')
    expect(html.match(/class="label"/g)).toHaveLength(2)
  })
})
