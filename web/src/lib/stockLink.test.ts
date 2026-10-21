import { describe, expect, it } from 'vitest'
import { productsLink, stockToProducts } from './stockLink'

describe('«Остатки» ведут на «Товары» (SPEC-30 §11)', () => {
  it('без отборов — просто «Товары»', () => {
    expect(stockToProducts('')).toBe('/products')
    expect(stockToProducts('?filter=all')).toBe('/products')
  })

  it('отбор, поиск, категория и вкладка «Брак» сохраняются', () => {
    expect(stockToProducts('?filter=low')).toBe('/products?stock=low')
    expect(stockToProducts('?low=true&q=лампа')).toBe('/products?stock=low&q=%D0%BB%D0%B0%D0%BC%D0%BF%D0%B0')
    expect(stockToProducts('?filter=stale&category_id=abc')).toBe('/products?stock=stale&category_id=abc')
    expect(stockToProducts('?tab=defects')).toBe('/products?tab=defects')
    expect(stockToProducts('?filter=чтото')).toBe('/products')
  })

  it('ссылки сводки — на нужный отбор', () => {
    expect(productsLink('low')).toBe('/products?stock=low')
    expect(productsLink()).toBe('/products')
  })
})
