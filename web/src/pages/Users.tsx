import { useState, type FormEvent } from 'react'
import { Badge, Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, Modal, PageHeader, Table } from '../components/ui'
import { get, patch, post } from '../lib/api'
import { useAction, useLoad } from '../lib/hooks'
import type { Role, UserRow } from '../lib/types'

const ROLE_LABELS: Record<Role, string> = {
  owner: 'Владелец',
  admin: 'Администратор',
}

const MIN_PASSWORD = 8

function RoleSelect({ value, onChange }: { value: Role; onChange: (r: Role) => void }) {
  return (
    <select value={value} onChange={(e) => onChange(e.target.value as Role)}>
      {(Object.keys(ROLE_LABELS) as Role[]).map((r) => (
        <option key={r} value={r}>
          {ROLE_LABELS[r]}
        </option>
      ))}
    </select>
  )
}

function EditModal({ user, onClose, onSaved }: { user: UserRow; onClose: () => void; onSaved: () => void }) {
  const [form, setForm] = useState({ full_name: user.full_name, role: user.role, active: user.active, password: '' })
  const { busy, error, run } = useAction()

  const save = () =>
    run(async () => {
      if (form.password && [...form.password].length < MIN_PASSWORD) throw new Error(`Пароль не короче ${MIN_PASSWORD} символов`)
      await patch<UserRow>(`/users/${user.id}`, {
        full_name: form.full_name,
        role: form.role,
        active: form.active,
        ...(form.password ? { password: form.password } : {}),
      })
      onSaved()
    })

  return (
    <Modal title={`Пользователь ${user.login}`} onClose={onClose}>
      <div className="flex flex-col gap-3">
        <Field label="Имя">
          <input value={form.full_name} onChange={(e) => setForm({ ...form, full_name: e.target.value })} />
        </Field>
        <Field label="Роль">
          <RoleSelect value={form.role} onChange={(role) => setForm({ ...form, role })} />
        </Field>
        <Field label="Новый пароль" hint="Оставьте пустым, чтобы не менять">
          <input
            type="password"
            autoComplete="new-password"
            value={form.password}
            onChange={(e) => setForm({ ...form, password: e.target.value })}
          />
        </Field>
        <Checkbox label="Активен" checked={form.active} onChange={(v) => setForm({ ...form, active: v })} />
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

const EMPTY = { login: '', full_name: '', password: '', role: 'admin' as Role }

export default function Users() {
  const list = useLoad(() => get<UserRow[]>('/users'), [])
  const [form, setForm] = useState(EMPTY)
  const [editing, setEditing] = useState<UserRow | null>(null)
  const { busy, error, run } = useAction()

  const create = (e: FormEvent) => {
    e.preventDefault()
    void run(async () => {
      if ([...form.password].length < MIN_PASSWORD) throw new Error(`Пароль не короче ${MIN_PASSWORD} символов`)
      await post<UserRow>('/users', form)
      setForm(EMPTY)
      list.reload()
    })
  }

  return (
    <div>
      <PageHeader title="Пользователи" />

      <Card className="mb-4">
        <form onSubmit={create} className="grid gap-3 sm:grid-cols-2 lg:grid-cols-[1fr_1.5fr_1fr_1fr_auto] lg:items-end">
          <Field label="Логин">
            <input autoComplete="off" value={form.login} onChange={(e) => setForm({ ...form, login: e.target.value })} />
          </Field>
          <Field label="Имя">
            <input value={form.full_name} onChange={(e) => setForm({ ...form, full_name: e.target.value })} />
          </Field>
          <Field label="Пароль">
            <input
              type="password"
              autoComplete="new-password"
              value={form.password}
              onChange={(e) => setForm({ ...form, password: e.target.value })}
            />
          </Field>
          <Field label="Роль">
            <RoleSelect value={form.role} onChange={(role) => setForm({ ...form, role })} />
          </Field>
          <Button type="submit" disabled={busy || !form.login.trim() || !form.full_name.trim() || !form.password}>
            Добавить
          </Button>
        </form>
        <p className="mt-2 text-xs text-slate-500">Пароль — не короче {MIN_PASSWORD} символов.</p>
        <div className="mt-2">
          <ErrorBox error={error} />
        </div>
      </Card>

      <Card>
        <ErrorBox error={list.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : !list.data?.length ? (
          <Empty>Пользователей нет</Empty>
        ) : (
          <Table head={['Логин', 'Имя', 'Роль', 'Статус']}>
            {list.data.map((u) => (
              <tr
                key={u.id}
                className={`cursor-pointer hover:bg-slate-50 ${u.active ? '' : 'text-slate-400'}`}
                onClick={() => setEditing(u)}
              >
                <td className="px-2 py-2 font-medium">{u.login}</td>
                <td className="px-2 py-2">{u.full_name}</td>
                <td className="px-2 py-2">{ROLE_LABELS[u.role]}</td>
                <td className="px-2 py-2">{u.active ? <Badge tone="green">активен</Badge> : <Badge>отключён</Badge>}</td>
              </tr>
            ))}
          </Table>
        )}
      </Card>

      {editing && (
        <EditModal
          user={editing}
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
