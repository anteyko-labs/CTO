import { describe, expect, it } from 'vitest'
import { DEFAULT_EXTRA, extraDocHtml } from './extraDocs'

const seller = {
  name: 'ИП Гайбулин',
  inn: '123',
  address: 'г. Балыкчы',
  phone: '',
  director: 'Гайбулин А. И.',
  bank: '',
  city: 'Балыкчы',
}
const png = 'data:image/png;base64,iVBORw0KGgo='

describe('другие документы точки (задача 017)', () => {
  it('реквизиты, дата и подпись руководителя подставляются', () => {
    const html = extraDocHtml('{{продавец}} в лице {{руководитель}}, {{дата}}\nРаботодатель: {{подпись_продавца}}', seller, png, '2026-11-02')
    expect(html).toContain('ИП Гайбулин в лице Гайбулин А. И., 02.11.2026')
    expect(html).toContain('<img src="data:image/png')
  })

  it('без подписи руководителя — линия; пустой реквизит — линия', () => {
    const html = extraDocHtml('{{телефон_продавца}} / {{подпись_продавца}}', seller, null)
    expect(html).toBe('________________ / ________________')
  })

  it('текст экранируется', () => {
    expect(extraDocHtml('<b>x</b>', seller, null)).toBe('&lt;b&gt;x&lt;/b&gt;')
  })

  it('шаблоны по умолчанию — договор об ответственности и трудовой, с подписью руководителя', () => {
    expect(DEFAULT_EXTRA.map((d) => d.title)).toEqual(['Договор об ответственности за товар на складе', 'Трудовой договор'])
    for (const d of DEFAULT_EXTRA) expect(d.text).toContain('{{подпись_продавца}} / {{руководитель}} /')
  })
})
