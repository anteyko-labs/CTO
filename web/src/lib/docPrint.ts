// Документы A4 (акт сверки, расписка, договор, остатки, отчёты) на кассе — на принтер HP (задача 034):
// браузер не выбирает принтер со страницы, а окно кассы без диалога печатает на чековый. Поэтому окно
// кассы (ярлык с `?docs=station`) отдаёт документ станции документов — своему окну Chrome с принтером
// HP. Остальные устройства печатают у себя, как раньше.
import { toast } from '../components/ui'
import { get, newOpId, post } from './api'
import { printInFrame } from './printFrame'

const KEY = 'avtodom.docs.station'

type Store = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>

/** `?docs=station` включает станцию документов на этом устройстве, `?docs=local` — выключает. */
export function rememberDocRoute(search: string, storage: Store): void {
  const docs = new URLSearchParams(search).get('docs')
  if (docs === 'station') storage.setItem(KEY, '1')
  if (docs === 'local') storage.removeItem(KEY)
}

export function docsGoToStation(storage: Pick<Storage, 'getItem'>): boolean {
  return storage.getItem(KEY) === '1'
}

/** Документ — на станцию. Станции нет — не печатаем здесь: A4 вышел бы на чековой ленте. */
async function toDocStation(title: string, html: string): Promise<void> {
  try {
    const list = await get<{ station_online: boolean }>('/label-jobs?size=doc')
    if (!list.station_online) {
      toast('Принтер HP: окно «Avtodom docs» не открыто — документ не напечатан', 'error')
      return
    }
    await post('/label-jobs/doc', { op_id: newOpId(), title, html })
    toast('Документ ушёл на принтер HP')
  } catch {
    toast('Принтер HP недоступен — документ не напечатан', 'error')
  }
}

/** Напечатать готовый документ A4. */
export async function printDoc(title: string, html: string): Promise<void> {
  if (!docsGoToStation(localStorage)) {
    printInFrame(html)
    return
  }
  await toDocStation(title, html)
}

/** Текущая страница со стилями — документом: отчёты печатаются самой страницей (`@media print`). */
export function pageHtml(title: string): string {
  const css = Array.from(document.styleSheets)
    .flatMap((sheet) => {
      try {
        return Array.from(sheet.cssRules).map((r) => r.cssText)
      } catch {
        return []
      }
    })
    .join('\n')
  const body = document.body.cloneNode(true) as HTMLElement
  body.querySelectorAll('script').forEach((el) => el.remove())
  return `<!doctype html><html lang="ru"><head><meta charset="utf-8"><title>${title}</title><style>${css}</style></head><body>${body.innerHTML}</body></html>`
}

/** Напечатать текущую страницу-отчёт. `title` — постоянная подпись из кода. */
export async function printPage(title: string): Promise<void> {
  if (!docsGoToStation(localStorage)) {
    window.print()
    return
  }
  await toDocStation(title, pageHtml(title))
}
