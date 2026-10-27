import { useEffect, useState } from 'react'
import {
  claimForeign,
  dropDone,
  foreignWaiting,
  isOwnItem,
  pendingCount,
  readSnapshotMeta,
  receiptOf,
  receiptsWord,
  rejectedItems,
  removeRejected,
  SNAPSHOT_EVENT,
  snapshotIsStale,
  syncOutbox,
  watchOutbox,
  type OutboxItem,
  type SnapshotMeta,
} from '../lib/offline'
import { useUser } from '../lib/auth'
import { formatDateTime, formatKg, formatLiters, formatSom } from '../lib/format'
import { useServerOnline } from '../lib/hooks'
import { printSale } from './salePrint'
import { Badge, Button, Modal, Table } from './ui'

const STATUS: Record<OutboxItem['status'], string> = {
  pending: 'ждёт отправки',
  sending: 'отправляется',
  done: 'отправлен',
  rejected: 'отклонён',
}

/** Строки чека коротко: «Масло 4 л ×1, Фильтр ×2». */
function linesText(i: OutboxItem): string {
  const lines = i.receipt?.lines ?? []
  if (lines.length === 0) return ''
  return lines.map((l) => `${l.name} ${l.kind === 'pour' ? formatLiters(l.qty) : l.kind === 'weight' ? formatKg(l.qty) : `×${l.qty}`}`).join(', ')
}

/** Состояние связи и очередь чеков: видно, что ещё не ушло на сервер и что сервер отклонил (SPEC-09). */
export function OfflineBar() {
  const user = useUser()
  const online = useServerOnline()
  const [items, setItems] = useState<OutboxItem[]>([])
  const [snap, setSnap] = useState<SnapshotMeta | null>(null)
  const [open, setOpen] = useState(false)
  const [confirmDrop, setConfirmDrop] = useState<string | null>(null)

  useEffect(() => {
    const stop = watchOutbox(setItems)
    // Только время и версия снимка, не весь снимок (SPEC-36 §16).
    const reread = () => void readSnapshotMeta().then(setSnap)
    reread()
    const t = setInterval(reread, 60_000)
    // Снимок только что сохранён — перечитываем сразу: плашка «устарел» не висит до минуты.
    window.addEventListener(SNAPSHOT_EVENT, reread)
    return () => {
      stop()
      clearInterval(t)
      window.removeEventListener(SNAPSHOT_EVENT, reread)
    }
  }, [])
  // Снимок обновился (в том числе подтверждён ответом 304) — перечитываем его вместе с очередью.
  useEffect(() => {
    void readSnapshotMeta().then(setSnap)
  }, [items, online])

  const own = items.filter((i) => isOwnItem(i, user.id))
  const waiting = pendingCount(own)
  const rejected = rejectedItems(own)
  const foreign = foreignWaiting(items, user.id)
  const stale = snapshotIsStale(snap)
  if (online && waiting === 0 && rejected.length === 0 && foreign.length === 0 && !stale) return null

  const reprint = (i: OutboxItem) => {
    const sale = receiptOf(i)
    if (sale) printSale(sale)
  }

  return (
    <>
      <button
        type="button"
        data-testid="offline-bar"
        className={`no-print w-full px-4 py-1.5 text-center text-xs ${
          !online ? 'bg-rose-600 text-white' : rejected.length > 0 ? 'bg-rose-100 text-rose-900' : 'bg-amber-100 text-amber-900'
        }`}
        onClick={() => setOpen(true)}
      >
        {online ? 'В сети' : 'Нет сети — касса работает, чеки уйдут позже'}
        {waiting > 0 && ` · не отправлено: ${waiting}`}
        {rejected.length > 0 && ` · отклонено сервером: ${rejected.length} — разберите`}
        {foreign.map((f) => ` · ${f.count} ${receiptsWord(f.count)} пользователя ${f.user_name || 'другого'} ждут его входа`).join('')}
        {stale && ' · снимок каталога устарел'}
      </button>
      {open && (
        <Modal title="Очередь чеков" onClose={() => setOpen(false)} wide>
          <div className="flex flex-col gap-3">
            <div className="text-sm text-slate-600">
              Снимок каталога: {snap ? formatDateTime(snap.saved_at) : 'нет'}
              {stale && ' — старше двух часов, цены и остатки могли измениться'}
            </div>
            {foreign.map((f) => (
              <div key={f.user_id} className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-900">
                <span>
                  {f.count} {receiptsWord(f.count)} пользователя <b>{f.user_name || 'другого'}</b> ждут его входа.
                </span>
                {user.role === 'owner' && (
                  <Button variant="secondary" onClick={() => void claimForeign(f.user_id, user).then(() => syncOutbox(true))}>
                    Отправить от себя
                  </Button>
                )}
              </div>
            ))}
            {rejected.length > 0 && (
              <div className="flex flex-col gap-2 rounded-md border border-rose-200 bg-rose-50 p-3" data-testid="rejected-list">
                <div className="text-sm font-medium text-rose-900">
                  Сервер не принял {rejected.length} {receiptsWord(rejected.length)}. Владелец уже получил уведомление; деньги и товар по ним
                  сверьте вручную.
                </div>
                {rejected.map((i) => (
                  <div key={i.op_id} className="flex flex-wrap items-start justify-between gap-2 rounded-md bg-white px-3 py-2 text-sm">
                    <div className="min-w-0 flex-1">
                      <div className="font-medium">
                        {i.temp_no} · {formatDateTime(i.created_at)}
                        {i.receipt && <> · {formatSom(i.receipt.total_tyiyn)}</>}
                      </div>
                      {linesText(i) && <div className="text-slate-600">{linesText(i)}</div>}
                      <div className="text-rose-800">Причина: {i.last_error || 'не указана'}</div>
                    </div>
                    <div className="flex shrink-0 gap-2">
                      {i.receipt && (
                        <Button variant="secondary" onClick={() => reprint(i)}>
                          Распечатать
                        </Button>
                      )}
                      {confirmDrop === i.op_id ? (
                        <Button
                          variant="ghost"
                          className="text-rose-700"
                          onClick={() => {
                            setConfirmDrop(null)
                            void removeRejected(i.op_id)
                          }}
                        >
                          Точно убрать?
                        </Button>
                      ) : (
                        <Button variant="ghost" onClick={() => setConfirmDrop(i.op_id)}>
                          Убрать
                        </Button>
                      )}
                    </div>
                  </div>
                ))}
              </div>
            )}
            {own.length === 0 ? (
              <div className="text-sm text-slate-500">Очередь пуста</div>
            ) : (
              <Table head={['Чек', 'Когда', 'Сумма', 'Состояние', 'Ошибка', '']}>
                {own.map((i) => (
                  <tr key={i.op_id}>
                    <td className="px-2 py-2 font-medium">{i.server_number ? `№ ${i.server_number}` : i.temp_no}</td>
                    <td className="whitespace-nowrap px-2 py-2">{formatDateTime(i.created_at)}</td>
                    <td className="whitespace-nowrap px-2 py-2">{i.receipt ? formatSom(i.receipt.total_tyiyn) : '—'}</td>
                    <td className="px-2 py-2">
                      <Badge tone={i.status === 'done' ? 'green' : i.status === 'rejected' ? 'rose' : 'amber'}>{STATUS[i.status]}</Badge>
                    </td>
                    <td className="px-2 py-2 text-slate-600">{i.last_error || '—'}</td>
                    <td className="px-2 py-2 text-right">
                      {i.receipt && (
                        <button type="button" className="text-sky-700 hover:underline" onClick={() => reprint(i)}>
                          Печать
                        </button>
                      )}
                    </td>
                  </tr>
                ))}
              </Table>
            )}
            <div className="text-xs text-slate-500">Отправленные чеки хранятся здесь неделю для повторной печати.</div>
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => void dropDone()}>
                Убрать отправленные
              </Button>
              <Button onClick={() => void syncOutbox(true)}>Отправить сейчас</Button>
            </div>
          </div>
        </Modal>
      )}
    </>
  )
}
