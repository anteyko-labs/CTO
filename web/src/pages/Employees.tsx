// Справочник сотрудников: кассиры и мастера без входа в систему (SPEC-02, инвариант 15).
import { useState, type FormEvent } from 'react'
import { Icon } from '../components/icons'
import { Badge, Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, Missing, Modal, PageHeader, RowMenu, Table, toast } from '../components/ui'
import { get, patch, post } from '../lib/api'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import type { Employee } from '../lib/types'

/** Роли сотрудника значками: «кассир», «мастер». */
function Roles({ e }: { e: Employee }) {
  if (!e.is_cashier && !e.is_master) return <span className="text-slate-400">—</span>
  return (
    <span className="flex flex-wrap gap-1">
      {e.is_cashier && <Badge tone="sky">кассир</Badge>}
      {e.is_master && <Badge tone="amber">мастер</Badge>}
    </span>
  )
}

function EditModal({ employee, onClose, onSaved }: { employee: Employee; onClose: () => void; onSaved: () => void }) {
  const [form, setForm] = useState(employee)
  const { busy, error, run } = useAction()

  const save = () =>
    run(async () => {
      await patch<Employee>(`/employees/${employee.id}`, {
        full_name: form.full_name,
        is_cashier: form.is_cashier,
        is_master: form.is_master,
        active: form.active,
      })
      onSaved()
    })

  return (
    <Modal title="Сотрудник" onClose={onClose}>
      <div className="flex flex-col gap-3">
        <Field label="ФИО">
          <input value={form.full_name} onChange={(e) => setForm({ ...form, full_name: e.target.value })} />
        </Field>
        <div className="flex flex-wrap gap-4">
          <Checkbox label="Кассир" checked={form.is_cashier} onChange={(v) => setForm({ ...form, is_cashier: v })} />
          <Checkbox label="Мастер" checked={form.is_master} onChange={(v) => setForm({ ...form, is_master: v })} />
          <Checkbox label="Активен" checked={form.active} onChange={(v) => setForm({ ...form, active: v })} />
        </div>
        <ErrorBox error={error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={busy || !form.full_name.trim()} onClick={() => void save()}>
            Сохранить
          </Button>
        </div>
      </div>
    </Modal>
  )
}

export default function Employees() {
  const list = useLoad(() => get<Employee[]>('/employees'), [])
  const [form, setForm] = useState({ full_name: '', is_cashier: false, is_master: true })
  const [adding, setAdding] = useState(false)
  const [q, setQ] = useState('')
  const [editing, setEditing] = useState<Employee | null>(null)
  const { busy, error, run } = useAction()
  const toggle = useAction()

  const create = (e: FormEvent) => {
    e.preventDefault()
    void run(async () => {
      await post<Employee>('/employees', form)
      setForm({ ...form, full_name: '' })
      setAdding(false)
      list.reload()
    })
  }

  const setActive = (e: Employee, active: boolean) =>
    void toggle.run(async () => {
      await patch<Employee>(`/employees/${e.id}`, { full_name: e.full_name, is_cashier: e.is_cashier, is_master: e.is_master, active })
      toast(active ? 'Сотрудник включён' : 'Сотрудник отключён')
      list.reload()
    })

  const notFilled = missingWithFocus(
    [Boolean(form.full_name.trim()), 'ФИО', '#employee-name'],
    [form.is_cashier || form.is_master, 'роль: кассир или мастер'],
  )
  const needle = q.trim().toLowerCase()
  const rows = (list.data ?? []).filter((e) => !needle || e.full_name.toLowerCase().includes(needle))

  return (
    <div>
      <PageHeader
        title="Сотрудники"
        actions={
          <Button onClick={() => setAdding(true)}>
            <Icon name="plus" /> Добавить
          </Button>
        }
      />

      <Card>
        {(list.data?.length ?? 0) > 5 && (
          <div className="mb-3">
            <input aria-label="Поиск сотрудника" placeholder="Поиск по ФИО" value={q} onChange={(e) => setQ(e.target.value)} />
          </div>
        )}
        <ErrorBox error={list.error ?? toggle.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : !list.data?.length ? (
          <Empty
            icon="employees"
            action={
              <Button onClick={() => setAdding(true)}>
                <Icon name="plus" /> Добавить сотрудника
              </Button>
            }
          >
            Сотрудников пока нет. Заведите кассира и мастера — их выбирают в чеке.
          </Empty>
        ) : rows.length === 0 ? (
          <Empty>Никого не найдено</Empty>
        ) : (
          <Table head={['ФИО', 'Роли', 'Статус', '']}>
            {rows.map((e) => (
              <tr
                key={e.id}
                className={`cursor-pointer hover:bg-slate-50 ${e.active ? '' : 'text-slate-400'}`}
                onClick={() => setEditing(e)}
              >
                <td className="px-2 py-2 font-medium">{e.full_name}</td>
                <td className="px-2 py-2">
                  <Roles e={e} />
                </td>
                <td className="px-2 py-2">{e.active ? <Badge tone="green">активен</Badge> : <Badge>отключён</Badge>}</td>
                <td className="overflow-visible! px-2 py-1 text-right" onClick={(ev) => ev.stopPropagation()}>
                  <RowMenu
                    items={[
                      { label: 'Изменить', onClick: () => setEditing(e) },
                      e.active
                        ? { label: 'Отключить', danger: true, onClick: () => setActive(e, false) }
                        : { label: 'Включить', onClick: () => setActive(e, true) },
                    ]}
                  />
                </td>
              </tr>
            ))}
          </Table>
        )}
      </Card>

      {adding && (
        <Modal title="Новый сотрудник" onClose={() => setAdding(false)}>
          <form onSubmit={create} className="flex flex-col gap-3">
            <Field label="ФИО" required>
              <input id="employee-name" autoFocus value={form.full_name} onChange={(e) => setForm({ ...form, full_name: e.target.value })} />
            </Field>
            <div className="flex flex-wrap gap-4">
              <Checkbox label="Кассир" checked={form.is_cashier} onChange={(v) => setForm({ ...form, is_cashier: v })} />
              <Checkbox label="Мастер" checked={form.is_master} onChange={(v) => setForm({ ...form, is_master: v })} />
            </div>
            <ErrorBox error={error} />
            <div className="flex flex-wrap items-center justify-end gap-2">
              <Missing items={notFilled} className="mr-auto" />
              <Button variant="secondary" onClick={() => setAdding(false)}>
                Отмена
              </Button>
              <Button type="submit" disabled={busy || notFilled.length > 0}>
                Добавить
              </Button>
            </div>
          </form>
        </Modal>
      )}

      {editing && (
        <EditModal
          employee={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null)
            list.reload()
          }}
        />
      )}
    </div>
  )
}
