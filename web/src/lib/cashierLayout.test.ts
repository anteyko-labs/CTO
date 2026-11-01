import { describe, expect, it } from 'vitest'
import { BUTTONS, DEFAULT_CASHIER, frequentPreset, layoutReset, move, placed, placeOf, setPlace, withDefaults } from './cashierLayout'

const owner = { role: 'owner' as const }
const admin = { role: 'admin' as const, permissions: ['debt_payment' as const, 'expenses' as const] }

describe('раскладка режима кассы (задача 003 Н2)', () => {
  it('без настройки — нынешний порядок', () => {
    const s = withDefaults(null)
    expect(placed(s, 'search', owner)).toEqual(['debt_payment', 'expense', 'battery_intake', 'battery_sell', 'staff_pay', 'no_receipt'])
    expect(placed(s, 'pay', owner)).toEqual(['delivery', 'comment', 'battery_pay', 'bonus', 'discount'])
    expect(placed(s, 'total', owner)).toEqual([])
    expect(placed(s, 'bar', owner)).toEqual(['shift_print', 'keyboard', 'fullscreen', 'exit'])
    expect(placed(s, 'more', owner)).toEqual([])
    expect(placed(s, 'hidden', owner)).toEqual(['pay_fix'])
    expect(s.tiles_dvh).toBe(24)
    expect(s.notes).toBe(1)
    expect(s.print_shift_profit).toBe(false)
    expect(s.sale_cost_on_till).toBe(false)
    // Продажа только при открытой смене — включена (задача 005 Н4).
    expect(s.sale_requires_shift).toBe(true)
    expect(withDefaults({ sale_requires_shift: false }).sale_requires_shift).toBe(false)
  })

  it('кнопка без права скрыта, где бы ни стояла', () => {
    const s = withDefaults(null)
    expect(placed(s, 'search', admin)).toEqual(['debt_payment', 'expense', 'battery_sell'])
    const moved = { ...s, buttons: setPlace(s.buttons, 'no_receipt', 'total') }
    expect(placed(moved, 'total', admin)).toEqual([])
    expect(placed(moved, 'total', owner)).toEqual(['no_receipt'])
  })

  it('«Наличные ↔ QR» — владельцу и администратору с правом, в любом месте и в полосе', () => {
    const s = withDefaults({ buttons: [{ id: 'pay_fix', place: 'bar' }] })
    expect(placed(s, 'bar', owner)).toEqual(['pay_fix', 'shift_print', 'keyboard', 'fullscreen', 'exit'])
    expect(placed(s, 'bar', admin)).not.toContain('pay_fix')
    const withRight = { role: 'admin' as const, permissions: ['till_pay_fix' as const] }
    expect(placed(s, 'bar', withRight)).toContain('pay_fix')
    const total = { ...s, buttons: setPlace(s.buttons, 'pay_fix', 'total') }
    expect(placed(total, 'total', withRight)).toEqual(['pay_fix'])
    // «Частое — у итога» её не трогает: по умолчанию скрыта.
    expect(placeOf(frequentPreset(DEFAULT_CASHIER), 'pay_fix')).toBe('hidden')
  })

  it('неизвестные и повторы отброшены, недостающие — на своих местах', () => {
    const s = withDefaults({
      buttons: [
        { id: 'bonus', place: 'total' },
        { id: 'bonus', place: 'more' },
        // @ts-expect-error — старая или чужая кнопка
        { id: 'print_money', place: 'search' },
        { id: 'exit', place: 'hidden' },
      ],
    })
    expect(s.buttons).toHaveLength(BUTTONS.length)
    expect(placeOf(s, 'bonus')).toBe('total')
    expect(placeOf(s, 'exit')).toBe('bar')
  })

  it('порядок стрелками — внутри места', () => {
    const b = DEFAULT_CASHIER.buttons
    expect(placed({ ...DEFAULT_CASHIER, buttons: move(b, 'expense', -1) }, 'search', owner).slice(0, 2)).toEqual(['expense', 'debt_payment'])
    // Первая в месте выше не поднимается; соседнее место не задевается.
    expect(move(b, 'debt_payment', -1)).toEqual(b)
    expect(placed({ ...DEFAULT_CASHIER, buttons: move(b, 'delivery', -1) }, 'pay', owner)).toEqual(['delivery', 'comment', 'battery_pay', 'bonus', 'discount'])
    expect(setPlace(b, 'keyboard', 'total')).toEqual(b)
  })

  it('набор «Частое — у итога» и «Как было»', () => {
    const f = frequentPreset({ ...DEFAULT_CASHIER, admin_limit_pct: 10 })
    expect(placed(f, 'total', owner)).toEqual(['discount', 'bonus', 'battery_pay'])
    expect(placed(f, 'search', owner)).toEqual([])
    expect(placed(f, 'more', owner)).toHaveLength(6)
    expect(f.submit_wide && f.exit_grey).toBe(true)
    expect(f.tiles_dvh).toBe(34)
    expect(f.admin_limit_pct).toBe(10)
    const back = layoutReset(f)
    expect(back.buttons).toEqual(DEFAULT_CASHIER.buttons)
    expect(back.submit_wide).toBe(false)
    expect(back.admin_limit_pct).toBe(10)
  })
})
