// Чек книжки для чека «В сервис», пробитого без сети (задача 003, хвост Н5): книжку можно собрать
// только из ответа сервера, поэтому её печатают, когда очередь отправила чек. Сама — по настройкам
// «Чек книжки — печатать сам» и «Печатать книжку для чеков без сети после отправки»; окна кассы не
// видно, печать выключена или не удалась — уведомление с кнопкой «Печать книжки». Касса открыта в
// нескольких вкладках — чек отправляет одна, о нём узнают все: печатает одна видимая, уведомление
// снимается во всех после печати или «✕».
import { useEffect, useRef, useState } from 'react'
import { get } from '../lib/api'
import { claimBookPrint, offlineBookDone, watchOfflineBooks, type SaleSent } from '../lib/offline'
import { printOilBook, saleCheckOf } from '../lib/oilBookPrint'
import { loadOilBookSettings, offlineBookPlan, offlineBookTab, type OilBookSettings } from '../lib/oilBookSettings'
import type { VehicleBook } from '../lib/oilBook'
import { Button } from './ui'

interface Pending {
  op_id: string
  number: number
  book: VehicleBook
  sent: SaleSent
  prefs: OilBookSettings
}

const print = (p: Pick<Pending, 'book' | 'sent' | 'prefs'>) =>
  printOilBook(p.book, { client: p.sent.sale.party_name ?? null, master: p.sent.sale.master_name ?? null, saleId: p.sent.sale.id, print: p.prefs })

export function OfflineBookPrints() {
  const [pending, setPending] = useState<Pending[]>([])
  // Книжки, которые уже напечатала или закрыла какая-то вкладка: уведомление о них не показываем.
  const finished = useRef(new Set<string>())

  useEffect(() => {
    const onDone = (op: string) => {
      finished.current.add(op)
      setPending((xs) => xs.filter((x) => x.op_id !== op))
    }
    const onSent = (sent: SaleSent) => {
      void (async () => {
        const prefs = await loadOilBookSettings()
        const plan = offlineBookPlan(sent.body, prefs, document.visibilityState === 'visible')
        if (plan === 'none') return
        const book = await get<VehicleBook>(`/vehicles/${String(sent.body.vehicle_id)}/oil-book`)
        // Печатать нечего (только работы без срока, только фильтры при выключенных фильтрах) —
        // как у чека при связи (ADR-130).
        if (!saleCheckOf(book, sent.sale.id, prefs)) return
        const item: Pending = { op_id: sent.op_id, number: sent.sale.number, book, sent, prefs }
        const step = await offlineBookTab(plan, {
          claim: () => claimBookPrint(item.op_id),
          print: () => print(item),
          finished: () => finished.current.has(item.op_id),
          done: () => offlineBookDone(item.op_id),
        })
        // Печать не удалась, окна не видно или печать выключена — остаётся кнопка.
        if (step === 'notice') setPending((xs) => (xs.some((x) => x.op_id === item.op_id) ? xs : [...xs, item]))
      })().catch(() => undefined)
    }
    return watchOfflineBooks(onSent, onDone)
  }, [])

  if (pending.length === 0) return null
  const drop = (op: string) => offlineBookDone(op)
  return (
    <div className="no-print fixed bottom-20 left-4 z-40 flex max-w-sm flex-col gap-2 md:bottom-6" data-testid="offline-book-prints">
      {pending.map((p) => (
        <div key={p.op_id} role="status" className="flex flex-wrap items-center gap-2 rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-900 shadow-lg">
          <span className="min-w-0 flex-1">Чек № {p.number}, пробитый без сети, отправлен. Книжка не напечатана.</span>
          <Button
            className="px-2 py-1 text-xs"
            onClick={() => {
              void print(p).then(() => drop(p.op_id))
            }}
          >
            Печать книжки
          </Button>
          <button type="button" aria-label="Скрыть" className="text-amber-700 hover:text-amber-900" onClick={() => drop(p.op_id)}>
            ✕
          </button>
        </div>
      ))}
    </div>
  )
}
