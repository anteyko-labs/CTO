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
  it('маленькая 30×20 — штрихкод и короткая цена «500 с/шт», без названия', async () => {
    const { labelsHtml } = await import('./labels')
    const html = labelsHtml([{ product, copies: 2 }], small)
    expect(html).toContain('size: 30mm 20mm')
    expect(html).toContain('data-code')
    expect(html).toContain('class="price big">500\u00a0с/шт<')
    expect(html).not.toContain('Фильтр масляный')
    expect(html).not.toContain('Mann')
    expect(html.match(/class="label"/g)).toHaveLength(2)
  })
})

describe('цена за литр на этикетке (задача 021)', () => {
  const oil = {
    ...product,
    id: 'p2',
    name: 'Super 3000 5W-30',
    brand: 'Mobil',
    unit: 'ml',
    container_ml: 4000,
    attrs: { viscosity: '5W-30' },
    sale_price_tyiyn: 200_000,
    pour_price_per_l_tyiyn: 50_000,
  } as unknown as Product
  it('большая — «Цена: 500 с за л», фильтр в том же задании — «за шт»', async () => {
    const { labelsHtml } = await import('./labels')
    const html = labelsHtml([{ product: oil, copies: 1 }, { product, copies: 1 }], big, 'liter')
    expect(html).toContain('Цена: 500\u00a0с за л')
    expect(html).toContain('Цена: 500\u00a0с за шт')
  })
  it('маленькая — коротко «500 с/л», без «Цена:»', async () => {
    const { labelsHtml } = await import('./labels')
    const html = labelsHtml([{ product: oil, copies: 1 }], small, 'liter')
    expect(html).toContain('class="price big">500\u00a0с/л<')
    expect(html).not.toContain('Цена:')
  })
  it('маленькая с названием — короткая цена в нижней строке', async () => {
    const { labelsHtml } = await import('./labels')
    const html = labelsHtml([{ product: oil, copies: 1 }], { ...small, show_name: true }, 'piece')
    expect(html).toContain('class="price">2\u00a0000\u00a0с/шт<')
  })
})
