// Касса ждёт подтверждения фирмы по чеку в долг (задача 039 C, ADR-202): опрос запроса, отказ с
// причиной, после таймаута — «Провести без подтверждения». Подтвердили — чек проводится тем же
// op_id (I7, I8).
import { useEffect, useState } from 'react'
import { Button, ErrorBox, Modal, Notice } from './ui'
import { get, post } from '../lib/api'
import { formatSom, formatDateTime } from '../lib/format'
import { useAction } from '../lib/hooks'

export interface SaleApproval {
  id: string | null
  status: 'not_needed' | 'pending' | 'approved' | 'rejected' | 'expired' | 'skipped'
  debt_tyiyn: number
  expires_at: string | null
  reason: string
  decided_by: string
  forced: boolean
  can_force: boolean
  can_post: boolean
}

/** Запрос фирме перед чеком; `can_post` — чек можно проводить (запрос не нужен или решён). */
export async function askApproval(body: unknown): Promise<SaleApproval> {
  return post<SaleApproval>('/sales/approvals', { draft: body })
}

export function SaleApprovalWait({ approval, onReady, onClose }: { approval: SaleApproval; onReady: () => void; onClose: () => void }) {
  const [a, setA] = useState(approval)
  const { run, busy, error } = useAction()
  const id = a.id
  const waiting = !a.can_post && a.status !== 'rejected'
  useEffect(() => {
    if (!id || !waiting) return
    const t = window.setInterval(() => {
      void get<SaleApproval>(`/sales/approvals/${id}`)
        .then(setA)
        .catch(() => undefined)
    }, 3000)
    return () => window.clearInterval(t)
  }, [id, waiting])
  useEffect(() => {
    if (a.can_post) onReady()
  }, [a.can_post, onReady])
  const force = () =>
    run(async () => {
      if (!id) return
      setA(await post<SaleApproval>(`/sales/approvals/${id}/force`, {}))
    })
  return (
    <Modal title="Подтверждение фирмы" onClose={onClose}>
      <div className="flex flex-col gap-3" data-testid="approval-wait">
        <div>В долг: {formatSom(a.debt_tyiyn)}</div>
        {a.status === 'rejected' ? (
          <Notice tone="rose">Фирма отклонила — чек не проведён{a.reason ? `. Причина: ${a.reason}` : ''}</Notice>
        ) : a.status === 'expired' ? (
          <Notice>Фирма не ответила вовремя. Можно провести без подтверждения — на чеке будет отметка.</Notice>
        ) : (
          <Notice tone="sky">
            Ждём подтверждения фирмы…{a.expires_at ? ` Можно будет провести без него после ${formatDateTime(a.expires_at)}.` : ''}
          </Notice>
        )}
        <ErrorBox error={error} />
        <div className="flex flex-wrap gap-2">
          {a.can_force && (
            <Button disabled={busy} onClick={() => void force()}>
              Провести без подтверждения
            </Button>
          )}
          <Button variant="secondary" onClick={onClose}>
            {a.status === 'rejected' ? 'Закрыть' : 'Отмена'}
          </Button>
        </div>
      </div>
    </Modal>
  )
}
