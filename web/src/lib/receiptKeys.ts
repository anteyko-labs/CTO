// Ввод прихода с клавиатуры (SPEC-29 §1): Enter переводит к следующему полю строки, с последнего —
// к количеству следующей строки, с последней строки — обратно в поиск.

export const LINE_FIELDS = ['count', 'price', 'markup', 'sale'] as const
export type LineField = (typeof LINE_FIELDS)[number]

/** Id поля строки `lineNo` (с 1). */
export const fieldId = (lineNo: number, field: LineField): string => `line-${lineNo}-${field}`

/** Куда перейти по Enter: id следующего поля или `null` — в поле поиска. */
export function nextField(lineNo: number, field: LineField, lineCount: number): string | null {
  const i = LINE_FIELDS.indexOf(field)
  if (i >= 0 && i < LINE_FIELDS.length - 1) return fieldId(lineNo, LINE_FIELDS[i + 1])
  return lineNo < lineCount ? fieldId(lineNo + 1, 'count') : null
}
