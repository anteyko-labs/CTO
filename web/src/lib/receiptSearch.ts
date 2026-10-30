// Список приходов (SPEC-03, задача 002, П6): параметры запроса. Без поиска — период (по умолчанию
// 30 дней), с поиском — за всё время: накладную ищут по номеру, поставщику, товару или сумме, а
// сумму в сомах сервер сравнивает с итогом в тыйынах.

export interface ReceiptsFilter {
  from: string
  to: string
  kind: string
  q: string
  photo: boolean
}

/** Параметры `GET /receipts`: пустые не отправляются. */
export function receiptsQuery(f: ReceiptsFilter): Record<string, string | boolean | undefined> {
  const q = f.q.trim()
  return {
    ...(q ? { q } : { from: f.from, to: f.to }),
    kind: f.kind,
    photo: f.photo || undefined,
  }
}
