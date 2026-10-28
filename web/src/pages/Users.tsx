// Пользователи со входом: владельцы и администраторы; экран только для владельца (SPEC-01).
// «Что можно» — права администратора, которые выбирает владелец (SPEC-30 §5, ADR-097).
import { useState, type FormEvent } from 'react'
import { Icon } from '../components/icons'
import { Badge, Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, Missing, Modal, PageHeader, RowMenu, Table, toast } from '../components/ui'
import { BotStartLink, startLink } from '../components/BotStartLink'
import { del, get, newOpId, patch, post, put } from '../lib/api'
import { formatDateTime } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import { PERMISSION_GROUPS, PERMISSIONS } from '../lib/permissions'
import type { Permission, Role, UserRow } from '../lib/types'

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
  const [form, setForm] = useState({
    full_name: user.full_name,
    role: user.role,
    active: user.active,
    password: '',
  })
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

/** «Что можно» администратору: флажки по группам, сохраняется списком целиком (SPEC-30 §5). */
function PermissionsModal({ user, onClose, onSaved }: { user: UserRow; onClose: () => void; onSaved: () => void }) {
  const [chosen, setChosen] = useState<Set<Permission>>(() => new Set(user.permissions ?? []))
  // Своя операция на каждый набор флажков: повтор после потерянного ответа не провёл бы другой набор.
  const [opId, setOpId] = useState(newOpId)
  const { busy, error, run } = useAction()
  const toggle = (key: Permission, on: boolean) => {
    const next = new Set(chosen)
    if (on) next.add(key)
    else next.delete(key)
    setChosen(next)
    setOpId(newOpId())
  }
  const save = () =>
    run(async () => {
      await put<UserRow>(`/users/${user.id}/permissions`, {
        op_id: opId,
        permissions: PERMISSIONS.map((p) => p.key).filter((k) => chosen.has(k)),
      })
      toast('Права сохранены')
      onSaved()
    })

  return (
    <Modal title={`Что можно: ${user.full_name}`} onClose={onClose} wide>
      <div className="flex flex-col gap-4" data-testid="permissions">
        <div className="text-sm text-slate-600">
          Владелец может всё. Отметьте, что можно этому администратору; без права пункты меню и кнопки у него скрыты, а сервер отвечает «Нет права». Себестоимость,
          прибыль, цены товаров, исправления документов и начисления — только у владельца.
        </div>
        <div className="grid gap-4 sm:grid-cols-2">
          {PERMISSION_GROUPS.map((group) => (
            <fieldset key={group} className="flex flex-col gap-2 rounded-md border border-slate-200 p-3">
              <legend className="px-1 text-sm font-medium">{group}</legend>
              {PERMISSIONS.filter((p) => p.group === group).map((p) => (
                <div key={p.key} className="flex flex-col">
                  <Checkbox label={p.label} checked={chosen.has(p.key)} onChange={(v) => toggle(p.key, v)} />
                  {p.hint && <div className="pl-6 text-xs text-slate-500">{p.hint}</div>}
                </div>
              ))}
            </fieldset>
          ))}
        </div>
        <ErrorBox error={error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={busy} onClick={() => void save()}>
            Сохранить
          </Button>
        </div>
      </div>
    </Modal>
  )
}

/** Сколько прав у администратора — в списке пользователей. */
function rightsText(u: UserRow): string {
  if (u.role === 'owner') return 'всё'
  const n = PERMISSIONS.filter((p) => (u.permissions ?? []).includes(p.key)).length
  return `${n} из ${PERMISSIONS.length}`
}

const EMPTY = { login: '', full_name: '', password: '', role: 'admin' as Role }

export default function Users() {
  const list = useLoad(() => get<UserRow[]>('/users'), [])
  const [form, setForm] = useState(EMPTY)
  const [adding, setAdding] = useState(false)
  const [editing, setEditing] = useState<UserRow | null>(null)
  const [rights, setRights] = useState<UserRow | null>(null)
  const [telegram, setTelegram] = useState<UserRow | null>(null)
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
          <Table head={['Логин', 'Имя', 'Роль', 'Что можно', 'Телеграм', 'Статус', '']}>
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
                <td className="px-2 py-2" onClick={(ev) => ev.stopPropagation()}>
                  {u.role === 'admin' ? (
                    <button type="button" className="text-sky-700 hover:underline" onClick={() => setRights(u)}>
                      {rightsText(u)}
                    </button>
                  ) : (
                    <span className="text-slate-500">{rightsText(u)}</span>
                  )}
                </td>
                <td className="px-2 py-2" data-testid="user-telegram">
                  {u.telegram_linked_at ? <Badge tone="green">привязан</Badge> : <span className="text-slate-400">—</span>}
                </td>
                <td className="px-2 py-2">{u.active ? <Badge tone="green">активен</Badge> : <Badge>отключён</Badge>}</td>
                <td className="overflow-visible! px-2 py-1 text-right" onClick={(ev) => ev.stopPropagation()}>
                  <RowMenu
                    items={[
                      { label: 'Изменить или сменить пароль', onClick: () => setEditing(u) },
                      ...(u.role === 'admin' ? [{ label: 'Что можно', onClick: () => setRights(u) }] : []),
                      // Фото накладных в бот от администратора (ADR-135): код даёт только владелец.
                      ...(u.role === 'admin' && u.active ? [{ label: 'Телеграм', onClick: () => setTelegram(u) }] : []),
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

      {telegram && (
        <TelegramModal
          user={telegram}
          onClose={() => setTelegram(null)}
          onChanged={() => {
            setTelegram(null)
            list.reload()
          }}
        />
      )}
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
            <Field label="Роль" hint={form.role === 'admin' ? 'Права — по умолчанию: всё, кроме скидки и перелива; поменять — «Что можно»' : undefined}>
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

      {rights && (
        <PermissionsModal
          user={rights}
          onClose={() => setRights(null)}
          onSaved={() => {
            setRights(null)
            list.reload()
          }}
        />
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

/** Телеграм администратора (ADR-135): код на 15 минут, ссылка и QR; «Отвязать». Его чат
 *  принимает только фото накладных. */
function TelegramModal({ user, onClose, onChanged }: { user: UserRow; onClose: () => void; onChanged: () => void }) {
  const act = useAction()
  const bot = useLoad(() => get<{ username: string | null }>('/telegram/bot'), [])
  const [code, setCode] = useState<{ code: string; expires_at: string } | null>(null)
  return (
    <Modal title={`Телеграм: ${user.full_name}`} onClose={onClose}>
      <div className="flex flex-col gap-3 text-sm" data-testid="user-telegram-modal">
        <div className="text-slate-600">
          Администратор с правом «Приход товара» присылает в бот фото накладных — они становятся черновиками прихода. Остального в его чате нет.
        </div>
        {user.telegram_linked_at && <div>Привязан {formatDateTime(user.telegram_linked_at)}</div>}
        {code ? (
          <div className="flex flex-col gap-2">
            <div className="text-3xl font-bold tracking-widest" data-testid="user-telegram-code">
              {code.code}
            </div>
            <div className="text-slate-600">
              Администратор открывает бота и отправляет: <code className="rounded bg-slate-100 px-1">/start {code.code}</code> — код действует до{' '}
              {formatDateTime(code.expires_at)}.
            </div>
            {bot.data?.username && <BotStartLink link={startLink(bot.data.username, code.code)} />}
          </div>
        ) : (
          <Button
            disabled={act.busy}
            data-testid="user-telegram-code-btn"
            onClick={() => void act.run(async () => setCode(await post<{ code: string; expires_at: string }>(`/users/${user.id}/telegram/code`, {})))}
          >
            {user.telegram_linked_at ? 'Привязать заново: код' : 'Код привязки'}
          </Button>
        )}
        <ErrorBox error={act.error} />
        <div className="flex justify-between gap-2">
          {user.telegram_linked_at ? (
            <Button
              variant="danger"
              disabled={act.busy}
              onClick={() =>
                void act.run(async () => {
                  await del(`/users/${user.id}/telegram`)
                  toast('Телеграм отвязан')
                  onChanged()
                })
              }
            >
              Отвязать
            </Button>
          ) : (
            <span />
          )}
          <Button variant="secondary" onClick={onClose}>
            Закрыть
          </Button>
        </div>
      </div>
    </Modal>
  )
}
