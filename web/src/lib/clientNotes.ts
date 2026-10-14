// Пометки клиента к машине из Телеграм-бота (SPEC-22): точка видит, но не правит.

export interface ClientNote {
  id: string
  text: string
  created_at: string
  /** Клиент удалил пометку в боте; такие сервер отдаёт только владельцу. */
  deleted: boolean
}

/** Действующие пометки, свежие сверху — как их видит клиент. */
export function activeNotes(notes: ClientNote[] | undefined): ClientNote[] {
  return (notes ?? []).filter((n) => !n.deleted)
}

/** Строка для кассы под выбранной машиной: «Пометки клиента: стук справа; заливать только 5W-30». */
export function cashierNotesText(notes: ClientNote[] | undefined): string | null {
  const list = activeNotes(notes)
  return list.length ? `Пометки клиента: ${list.map((n) => n.text.trim()).join('; ')}` : null
}
