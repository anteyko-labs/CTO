import { useEffect, useState } from 'react'
import { dropDone, pendingCount, readSnapshot, snapshotIsStale, syncOutbox, watchOutbox, type OutboxItem, type Snapshot } from '../lib/offline'
import { formatDateTime } from '../lib/format'
import { Badge, Button, Modal, Table } from './ui'

const STATUS: Record<OutboxItem['status'], string> = {
  pending: 'ждёт отправки',
  sending: 'отправляется',
  done: 'отправлен',
  rejected: 'отклонён',
}

/** Состояние связи и очередь чеков: видно, что ещё не ушло на сервер (SPEC-09). */
export function OfflineBar() {
  const [online, setOnline] = useState(navigator.onLine)
  const [items, setItems] = useState<OutboxItem[]>([])
  const [snap, setSnap] = useState<Snapshot | null>(null)
  const [open, setOpen] = useState(false)

  useEffect(() => {
    const on = () => setOnline(true)
    const off = () => setOnline(false)
    window.addEventListener('online', on)
    window.addEventListener('offline', off)
    const stop = watchOutbox(setItems)
    void readSnapshot().then(setSnap)
    const t = setInterval(() => void readSnapshot().then(setSnap), 60_000)
    return () => {
      window.removeEventListener('online', on)
      window.removeEventListener('offline', off)
      stop()
      clearInterval(t)
    }
  }, [])

  const waiting = pendingCount(items)
  const stale = snapshotIsStale(snap)
  if (online && waiting === 0 && !stale) return null

  return (
    <>
      <button
        type="button"
        className={`no-print w-full px-4 py-1.5 text-center text-xs ${online ? 'bg-amber-100 text-amber-900' : 'bg-rose-600 text-white'}`}
        onClick={() => setOpen(true)}
      >
        {online ? 'В сети' : 'Нет сети — касса работает, чеки уйдут позже'}
        {waiting > 0 && ` · не отправлено: ${waiting}`}
        {stale && ' · снимок каталога устарел'}
      </button>
      {open && (
        <Modal title="Очередь чеков" onClose={() => setOpen(false)} wide>
          <div className="flex flex-col gap-3">
            <div className="text-sm text-slate-600">
              Снимок каталога: {snap ? formatDateTime(snap.saved_at) : 'нет'}
              {stale && ' — старше двух часов, цены и остатки могли измениться'}
            </div>
            {items.length === 0 ? (
              <div className="text-sm text-slate-500">Очередь пуста</div>
            ) : (
              <Table head={['Чек', 'Когда', 'Состояние', 'Ошибка']}>
                {items.map((i) => (
                  <tr key={i.op_id}>
                    <td className="px-2 py-2 font-medium">{i.server_number ? `№ ${i.server_number}` : i.temp_no}</td>
                    <td className="whitespace-nowrap px-2 py-2">{formatDateTime(i.created_at)}</td>
                    <td className="px-2 py-2">
                      <Badge tone={i.status === 'done' ? 'green' : i.status === 'rejected' ? 'rose' : 'amber'}>
                        {STATUS[i.status]}
                      </Badge>
                    </td>
                    <td className="px-2 py-2 text-slate-600">{i.last_error || '—'}</td>
                  </tr>
                ))}
              </Table>
            )}
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => void dropDone()}>
                Убрать отправленные
              </Button>
              <Button onClick={() => void syncOutbox()}>Отправить сейчас</Button>
            </div>
          </div>
        </Modal>
      )}
    </>
  )
}
