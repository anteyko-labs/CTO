// Справочник сотрудников: кассиры и мастера без входа в систему (SPEC-02, инвариант 15).
import { useState, type FormEvent } from 'react'
import { Badge, Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, Missing, Modal, PageHeader, Table } from '../components/ui'
import { get, patch, post } from '../lib/api'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import type { Employee } from '../lib/types'

const yes = (v: boolean) => (v ? 'да' : '—')

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
  const [editing, setEditing] = useState<Employee | null>(null)
  const { busy, error, run } = useAction()

  const create = (e: FormEvent) => {
    e.preventDefault()
    void run(async () => {
      await post<Employee>('/employees', form)
      setForm({ ...form, full_name: '' })
      list.reload()
    })
  }

  const notFilled = missingWithFocus(
    [Boolean(form.full_name.trim()), 'ФИО', '#employee-name'],
    [form.is_cashier || form.is_master, 'роль: кассир или мастер'],
  )

  return (
    <div>
      <PageHeader title="Сотрудники" />

      <Card className="mb-4">
        <form onSubmit={create} className="grid gap-3 sm:grid-cols-[2fr_auto_auto] sm:items-end">
          <Field label="ФИО" required>
            <input id="employee-name" value={form.full_name} onChange={(e) => setForm({ ...form, full_name: e.target.value })} />
          </Field>
          <div className="flex flex-wrap gap-4 sm:pb-2">
            <Checkbox label="Кассир" checked={form.is_cashier} onChange={(v) => setForm({ ...form, is_cashier: v })} />
            <Checkbox label="Мастер" checked={form.is_master} onChange={(v) => setForm({ ...form, is_master: v })} />
          </div>
          <Button type="submit" disabled={busy || notFilled.length > 0}>
            Добавить
          </Button>
        </form>
        <div className="mt-2 flex flex-col gap-2">
          <Missing items={notFilled} />
          <ErrorBox error={error} />
        </div>
      </Card>

      <Card>
        <ErrorBox error={list.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : !list.data?.length ? (
          <Empty>Сотрудников пока нет. Заведите кассира и мастера — их выбирают в чеке.</Empty>
        ) : (
          <Table head={['ФИО', 'Кассир', 'Мастер', 'Статус']}>
            {list.data.map((e) => (
              <tr
                key={e.id}
                className={`cursor-pointer hover:bg-slate-50 ${e.active ? '' : 'text-slate-400'}`}
                onClick={() => setEditing(e)}
              >
                <td className="px-2 py-2 font-medium">{e.full_name}</td>
                <td className="px-2 py-2">{yes(e.is_cashier)}</td>
                <td className="px-2 py-2">{yes(e.is_master)}</td>
                <td className="px-2 py-2">{e.active ? <Badge tone="green">активен</Badge> : <Badge>отключён</Badge>}</td>
              </tr>
            ))}
          </Table>
        )}
      </Card>

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
