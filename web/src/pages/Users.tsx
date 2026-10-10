// Пользователи со входом: владельцы и администраторы; экран только для владельца (SPEC-01).
import { useState, type FormEvent } from 'react'
import { Icon } from '../components/icons'
import { Badge, Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, Missing, Modal, PageHeader, RowMenu, Table, toast } from '../components/ui'
import { get, patch, post } from '../lib/api'
import { missingWithFocus } from '../lib/forms'
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
  const [adding, setAdding] = useState(false)
  const [editing, setEditing] = useState<UserRow | null>(null)
  const { busy, error, run } = useAction()
  const toggle = useAction()

  const create = (e: FormEvent) => {
    e.preventDefault()
    void run(async () => {
      if ([...form.password].length < MIN_PASSWORD) throw new Error(`Пароль не короче ${MIN_PASSWORD} символов`)
      await post<UserRow>('/users', form)
      setForm(EMPTY)
      setAdding(false)
      list.reload()
    })
  }

  const setActive = (u: UserRow, active: boolean) =>
    void toggle.run(async () => {
      await patch<UserRow>(`/users/${u.id}`, { full_name: u.full_name, role: u.role, active })
      toast(active ? 'Вход включён' : 'Вход отключён')
      list.reload()
    })

  const notFilled = missingWithFocus(
    [Boolean(form.login.trim()), 'логин', '#user-login'],
    [Boolean(form.full_name.trim()), 'имя', '#user-name'],
    [form.password.length >= MIN_PASSWORD, `пароль не короче ${MIN_PASSWORD} символов`, '#user-password'],
  )

  return (
    <div>
      <PageHeader
        title="Пользователи"
        subtitle="Кто входит в программу: владельцы и администраторы"
        actions={
          <Button onClick={() => setAdding(true)}>
            <Icon name="plus" /> Добавить
          </Button>
        }
      />

      <Card>
        <ErrorBox error={list.error ?? toggle.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : !list.data?.length ? (
          <Empty>Пользователей нет</Empty>
        ) : (
          <Table head={['Логин', 'Имя', 'Роль', 'Статус', '']}>
            {list.data.map((u) => (
              <tr
                key={u.id}
                className={`cursor-pointer hover:bg-slate-50 ${u.active ? '' : 'text-slate-400'}`}
                onClick={() => setEditing(u)}
              >
                <td className="px-2 py-2 font-medium">{u.login}</td>
                <td className="px-2 py-2">{u.full_name}</td>
                <td className="px-2 py-2">
                  <Badge tone={u.role === 'owner' ? 'amber' : 'sky'}>{ROLE_LABELS[u.role].toLowerCase()}</Badge>
                </td>
                <td className="px-2 py-2">{u.active ? <Badge tone="green">активен</Badge> : <Badge>отключён</Badge>}</td>
                <td className="overflow-visible! px-2 py-1 text-right" onClick={(ev) => ev.stopPropagation()}>
                  <RowMenu
                    items={[
                      { label: 'Изменить или сменить пароль', onClick: () => setEditing(u) },
                      u.active
                        ? { label: 'Отключить вход', danger: true, onClick: () => setActive(u, false) }
                        : { label: 'Включить вход', onClick: () => setActive(u, true) },
                    ]}
                  />
                </td>
              </tr>
            ))}
          </Table>
        )}
      </Card>

      {adding && (
        <Modal title="Новый пользователь" onClose={() => setAdding(false)}>
          {/* Чего не хватает для создания — рядом с кнопкой (docs/tier-3/ui-rules.md). */}
          <form onSubmit={create} className="flex flex-col gap-3">
            <Field label="Логин" required>
              <input id="user-login" autoFocus autoComplete="off" value={form.login} onChange={(e) => setForm({ ...form, login: e.target.value })} />
            </Field>
            <Field label="Имя" required>
              <input id="user-name" value={form.full_name} onChange={(e) => setForm({ ...form, full_name: e.target.value })} />
            </Field>
            <Field label="Пароль" required hint={`Не короче ${MIN_PASSWORD} символов`}>
              <input
                id="user-password"
                type="password"
                autoComplete="new-password"
                value={form.password}
                onChange={(e) => setForm({ ...form, password: e.target.value })}
              />
            </Field>
            <Field label="Роль">
              <RoleSelect value={form.role} onChange={(role) => setForm({ ...form, role })} />
            </Field>
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
