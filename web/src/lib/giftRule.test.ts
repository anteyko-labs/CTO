import { describe, expect, it } from 'vitest'
import { giftMinText, giftQtys, giftRuleStatus } from './giftRule'
import type { GiftRule } from './types'

const rule = (patch: Partial<GiftRule>): GiftRule => ({
  id: 'r',
  trigger_product_id: 'oil',
  trigger_name: 'Масло 5W-30',
  trigger_unit: 'ml',
  min_units: 3_500,
  active: false,
  needs_fix: false,
  items: [{ gift_product_id: 'f', name: 'Фильтр', gift_qty: 2 }],
  ...patch,
})

describe('правило подарка', () => {
  it('статус: включено, выключено, поправьте порог', () => {
    expect(giftRuleStatus(rule({ active: true }))).toBe('включено')
    expect(giftRuleStatus(rule({}))).toBe('выключено')
    expect(giftRuleStatus(rule({ needs_fix: true }))).toBe('выключено — поправьте порог')
  })

  it('порог в форму: литры у масла, штуки у остального, пусто без порога', () => {
    expect(giftMinText(rule({}))).toBe('3,5')
    expect(giftMinText(rule({ trigger_unit: 'piece', min_units: 2 }))).toBe('2')
    expect(giftMinText(rule({ min_units: 0 }))).toBe('')
    // Условие в кг и м — порог тысячными (ADR-178).
    expect(giftMinText(rule({ trigger_unit: 'g', min_units: 2500 }))).toBe('2,5')
    expect(giftMinText(rule({ trigger_unit: 'mm', min_units: 1500 }))).toBe('1,5')
  })

  it('количество подарков сохраняется при правке', () => {
    expect(giftQtys(rule({}))).toEqual({ f: 2 })
  })
})
