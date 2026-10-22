// Проверка заполненности форм. Интерфейс сам говорит, чего не хватает,
// а сервер те же правила проверяет заново (docs/tier-3/ui-rules.md).

/** Один пункт проверки: поле заполнено или нет, как его назвать человеку, куда поставить фокус
 *  и короткое название для строки «Не хватает: …» (режим кассы). */
export type Check = [filled: boolean, label: string, focus?: string, short?: string]

/** Названия незаполненного в порядке полей на экране. */
export function missing(...checks: Check[]): string[] {
  return checks.filter(([filled]) => !filled).map(([, label]) => label)
}

/** Незаполненное вместе с подсказкой, какое поле ставить в фокус. */
export function missingWithFocus(...checks: Check[]): { label: string; focus?: string; short?: string }[] {
  return checks.filter(([filled]) => !filled).map(([, label, focus, short]) => ({ label, focus, ...(short ? { short } : {}) }))
}

/** Переводит фокус на поле по его id или data-атрибуту и прокручивает к нему. */
export function focusField(selector: string): void {
  const el = document.querySelector<HTMLElement>(selector)
  if (!el) return
  el.scrollIntoView({ block: 'center', behavior: 'smooth' })
  el.focus()
}
