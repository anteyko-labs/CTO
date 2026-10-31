// Повтор того же запроса, когда ответа сервера не было (обрыв, тайм-аут, прокси без сервера):
// тело уходит то же — с тем же `op_id`, и сервер не прибавит полку дважды (ADR-062, задача 003,
// Н14). Новый ввод человека — новый `op_id`: тем же повторяется только автоматический повтор.
import { noServerAnswer } from './api'

const pause = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

/** `send` — один и тот же запрос; повторов — `tries` (по умолчанию 2), с паузой между ними. */
export async function retrySame<T>(send: () => Promise<T>, tries = 2, waitMs = 800, wait: (ms: number) => Promise<void> = pause): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    try {
      return await send()
    } catch (e) {
      if (attempt >= tries || !noServerAnswer(e)) throw e
      await wait(waitMs * (attempt + 1))
    }
  }
}
