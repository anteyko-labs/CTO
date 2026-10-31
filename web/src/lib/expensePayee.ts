// «Кому» у расхода (задача 003 Н12): как показывать поле и подсказки, предупреждение о двойном
// учёте зарплаты (ADR-023: оплата работникам — выплатой в «Расчёте», а не статьёй расхода).
import type { ExpensePayees } from './types'

/** Поле «Кому» не заполнено, а настройка требует его. */
export function payeeMissing(mode: ExpensePayees['mode'] | undefined, payee: string): boolean {
  return mode === 'required' && payee.trim() === ''
}

/** Подсказки к полю: без пустых и повторов (без учёта регистра), не больше `limit`. */
export function payeeHints(names: string[], limit = 50): string[] {
  const seen = new Set<string>()
  const out: string[] = []
  for (const n of names) {
    const t = n.trim()
    const key = t.toLocaleLowerCase('ru')
    if (!t || seen.has(key)) continue
    seen.add(key)
    out.push(t)
    if (out.length >= limit) break
  }
  return out
}

/** Название статьи похоже на оплату работникам — расход по ней посчитает зарплату второй раз. */
export function payrollLikeArticle(name: string): boolean {
  const n = name.toLocaleLowerCase('ru').replace(/ё/g, 'е').replace(/[.\s]+/g, ' ')
  return /зарплат|оклад|аванс|доп доход|премия работ/.test(n)
}

export const PAYROLL_ARTICLE_WARNING =
  'Оплату работникам проводите выплатой в «Расчёте» — она уже входит в «Зарплату работникам» отчёта о прибыли. Расход по такой статье уменьшит прибыль второй раз.'
