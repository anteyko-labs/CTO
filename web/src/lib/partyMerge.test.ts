import { describe, expect, it } from 'vitest'
import { becomesAgents, mergedBalance } from './partyMerge'

describe('объединение карточек', () => {
  it('складывает долги и аванс, основную не считает дважды', () => {
    const main = { id: 'a', balance_tyiyn: 30000 }
    expect(mergedBalance(main, [main, { id: 'b', balance_tyiyn: 50000 }, { id: 'c', balance_tyiyn: -15000 }])).toBe(65000)
    expect(mergedBalance(main, [])).toBe(30000)
  })
  it('люди становятся агентами только у фирмы', () => {
    const picked = [
      { kind: 'person', name: 'Усенов Нурлан' },
      { kind: 'company', name: 'IQOS ОсОО' },
    ]
    expect(becomesAgents('company', picked)).toEqual(['Усенов Нурлан'])
    expect(becomesAgents('person', picked)).toEqual([])
  })
})
