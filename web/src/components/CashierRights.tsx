// «Что можно на кассе» у кассира без входа (задача 003 Н10, ADR-{B8.1}). Только владелец.
// Пока открыта смена этого кассира, у вошедшего администратора действует пересечение прав входа
// и кассира: снятое здесь сервер не пропускает («Нет права у кассира …»). По умолчанию — «как у входа».
import { useState } from 'react'
import { get, newOpId, put } from '../lib/api'
import { useAction, useLoad } from '../lib/hooks'
import { CASHIER_GROUPS, CASHIER_PERMISSIONS } from '../lib/permissions'
import type { Employee } from '../lib/types'
import { Badge, Button, Checkbox, ErrorBox, toast } from './ui'

interface CashierRightsOut {
  employee_id: string
  full_name: string
  keys: string[]
  denied: string[]
}

export function CashierRights({ employee }: { employee: Employee }) {
  const rights = useLoad(() => get<CashierRightsOut>(`/employees/${employee.id}/permissions`), [employee.id])
  const [edited, setEdited] = useState<string[] | null>(null)
  // op_id формы: повтор после потерянного ответа не запишет журнал дважды, после успеха — новый.
  const [opId, setOpId] = useState(newOpId)
  const act = useAction()
  const saved = rights.data?.denied ?? []
  const denied = edited ?? saved
  const changed = edited !== null && JSON.stringify([...edited].sort()) !== JSON.stringify([...saved].sort())

  const toggle = (key: string, allowed: boolean) =>
    setEdited(allowed ? denied.filter((k) => k !== key) : [...denied.filter((k) => k !== key), key])

  const save = () =>
    void act.run(async () => {
      await put<CashierRightsOut>(`/employees/${employee.id}/permissions`, { op_id: opId, denied })
      setOpId(newOpId())
      setEdited(null)
      rights.reload()
      toast(denied.length ? 'Права кассира сохранены' : 'Кассиру — как у входа')
    })

  return (
    <div className="flex flex-col gap-2 border-t border-slate-200 pt-3" data-testid="cashier-rights">
      <div className="flex items-center justify-between gap-2">
        <h3 className="font-semibold">Что можно на кассе</h3>
        {saved.length === 0 ? <Badge tone="green">как у входа</Badge> : <Badge tone="amber">снято: {saved.length}</Badge>}
      </div>
      <div className="text-xs text-slate-500">
        Пока открыта смена этого кассира, на кассе можно только отмеченное — и только то, что можно входу администратора. Снимите
        флажок, и кассир этого не сделает, даже если вход это разрешает. Вас как владельца это не ограничивает.
      </div>
      {rights.data &&
        CASHIER_GROUPS.map((group) => (
          <div key={group} className="flex flex-col gap-1">
            <div className="text-xs font-medium uppercase text-slate-500">{group}</div>
            {CASHIER_PERMISSIONS.filter((p) => p.group === group && rights.data?.keys.includes(p.key)).map((p) => (
              <Checkbox key={p.key} label={p.label} checked={!denied.includes(p.key)} onChange={(v) => toggle(p.key, v)} />
            ))}
          </div>
        ))}
      <ErrorBox error={act.error ?? rights.error} />
      <div className="flex flex-wrap justify-end gap-2">
        {denied.length > 0 && (
          <Button variant="ghost" disabled={act.busy} onClick={() => setEdited([])}>
            Как у входа
          </Button>
        )}
        <Button disabled={act.busy || !changed} onClick={save}>
          Сохранить права
        </Button>
      </div>
    </div>
  )
}
