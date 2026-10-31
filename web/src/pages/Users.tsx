// Пользователи со входом: владельцы и администраторы; экран только для владельца (SPEC-01).
// «Что можно» — права администратора, которые выбирает владелец (SPEC-30 §5, ADR-097, задача 003 Н8).
import { useState, type FormEvent } from 'react'
import { Icon } from '../components/icons'
import { Badge, Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, Missing, Modal, PageHeader, RowMenu, Table, toast } from '../components/ui'
import { AdminTelegramModal, noReceiptsRight } from '../components/AdminTelegramModal'
import { get, newOpId, patch, post, put } from '../lib/api'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import { useUser } from '../lib/auth'
import { OWNER_ONLY, PERMISSION_GROUPS, PERMISSION_PRESETS, PERMISSIONS, presetOf, toggleGroup } from '../lib/permissions'
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
  const me = useUser()
  // Филиал администратора — у владельца нескольких филиалов (SPEC-33); список — открытые филиалы.
  const branches = (me.branches ?? []).filter((b) => !b.archived)
  const [form, setForm] = useState({
    login: user.login,
    full_name: user.full_name,
    role: user.role,
    active: user.active,
    password: '',
    branch_id: me.branch_id,
  })
  const { busy, error, run } = useAction()
  const moving = form.role === 'admin' && form.branch_id !== me.branch_id

  const save = () =>
    run(async () => {
      if (form.password && [...form.password].length < MIN_PASSWORD) throw new Error(`Пароль не короче ${MIN_PASSWORD} символов`)
      await patch<UserRow>(`/users/${user.id}`, {
        full_name: form.full_name,
        role: form.role,
        active: form.active,
        // Логин меняется (задача 003 Н8 и Н10): занятый — ошибка «логин занят».
        ...(form.login.trim() !== user.login ? { login: form.login.trim() } : {}),
        ...(form.password ? { password: form.password } : {}),
        ...(moving ? { branch_id: form.branch_id } : {}),
      })
      toast(moving ? 'Администратор переведён в другой филиал' : 'Сохранено')
      onSaved()
    })

  return (
    <Modal title={`Пользователь ${user.login}`} onClose={onClose}>
      <div className="flex flex-col gap-3">
        <Field label="Логин" hint="Новым логином входят со следующего входа; пароль прежний">
          <input id="user-edit-login" data-testid="user-edit-login" autoComplete="off" value={form.login} onChange={(e) => setForm({ ...form, login: e.target.value })} />
        </Field>
        <Field label="Имя">
          <input value={form.full_name} onChange={(e) => setForm({ ...form, full_name: e.target.value })} />
        </Field>
        <Field label="Роль">
          <RoleSelect value={form.role} onChange={(role) => setForm({ ...form, role })} />
        </Field>
        {me.several_branches && form.role === 'admin' && branches.length > 1 && (
          <Field label="Филиал" hint={moving ? 'Права переходят вместе с ним; Телеграм нужно привязать заново' : 'Где работает администратор'}>
            <select data-testid="user-edit-branch" value={form.branch_id} onChange={(e) => setForm({ ...form, branch_id: e.target.value })}>
              {branches.map((b) => (
                <option key={b.id} value={b.id}>
                  {b.name}
                </option>
              ))}
            </select>
          </Field>
        )}
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
          <Button disabled={busy || !form.full_name.trim() || !form.login.trim()} onClick={() => void save()}>
            Сохранить
          </Button>
        </div>
      </div>
    </Modal>
  )
}

/** «Что можно» администратору (SPEC-30 §5, задача 003 Н8): готовые наборы и «Как у…», группы целиком,
 *  что открывает каждое право, что остаётся только у владельца. Сохраняется списком целиком. */
function PermissionsModal({ user, others, onClose, onSaved }: { user: UserRow; others: UserRow[]; onClose: () => void; onSaved: () => void }) {
  const [chosen, setChosen] = useState<Permission[]>(() => presetOf(user.permissions ?? []))
  // Своя операция на каждый набор флажков: повтор после потерянного ответа не провёл бы другой набор.
  const [opId, setOpId] = useState(newOpId)
  const { busy, error, run } = useAction()
  const choose = (next: readonly string[]) => {
    setChosen(presetOf(next))
    setOpId(newOpId())
  }
  const toggle = (key: Permission, on: boolean) => choose(on ? [...chosen, key] : chosen.filter((k) => k !== key))
  const peers = others.filter((o) => o.role === 'admin' && o.id !== user.id)
  const save = () =>
    run(async () => {
      await put<UserRow>(`/users/${user.id}/permissions`, { op_id: opId, permissions: chosen })
      toast('Права сохранены')
      onSaved()
    })

  return (
    <Modal title={`Что можно: ${user.full_name}`} onClose={onClose} wide>
      <div className="flex flex-col gap-4" data-testid="permissions">
        <div className="text-sm text-slate-600">
          Владелец может всё. Отметьте, что можно этому администратору: без права пункты меню и кнопки у него скрыты, а сервер отвечает «Нет права».
          Изменения доходят до открытой кассы за минуту, без перезагрузки. Каждому кассиру можно завести свой вход со своими правами.
        </div>
        <div className="flex flex-wrap items-center gap-2" data-testid="permission-presets">
          <span className="text-sm text-slate-500">Набор:</span>
          {PERMISSION_PRESETS.map((p) => (
            <Button key={p.key} variant="secondary" title={p.hint} onClick={() => choose(p.permissions)}>
              {p.label}
            </Button>
          ))}
          {peers.length > 0 && (
            <select
              aria-label="Как у другого администратора"
              data-testid="permission-copy"
              value=""
              onChange={(e) => {
                const peer = peers.find((o) => o.id === e.target.value)
                if (peer) choose(peer.permissions ?? [])
              }}
            >
              <option value="">Как у…</option>
              {peers.map((o) => (
                <option key={o.id} value={o.id}>
                  {o.full_name} ({o.login})
                </option>
              ))}
            </select>
          )}
          <span className="ml-auto text-sm text-slate-500" data-testid="permission-count">
            {chosen.length} из {PERMISSIONS.length}
          </span>
        </div>
        <div className="grid gap-4 sm:grid-cols-2">
          {PERMISSION_GROUPS.map((group) => {
            const list = PERMISSIONS.filter((p) => p.group === group)
            const all = list.every((p) => chosen.includes(p.key))
            return (
              <fieldset key={group} className="flex flex-col gap-2 rounded-md border border-slate-200 p-3">
                <legend className="px-1 text-sm font-medium">{group}</legend>
                <div className="flex justify-end">
                  <button
                    type="button"
                    className="text-xs text-sky-700 hover:underline"
                    data-testid={`permission-group-${group}`}
                    onClick={() => choose(toggleGroup(chosen, group, !all))}
                  >
                    {all ? 'Выключить всё' : 'Включить всё'}
                  </button>
                </div>
                {list.map((p) => (
                  <div key={p.key} className="flex flex-col">
                    <Checkbox label={p.label} checked={chosen.includes(p.key)} onChange={(v) => toggle(p.key, v)} />
                    {p.hint && <div className="pl-6 text-xs text-slate-500">{p.hint}</div>}
                    {p.opens && <div className="pl-6 text-xs text-slate-400">Открывает: {p.opens}</div>}
                  </div>
                ))}
              </fieldset>
            )
          })}
          <div className="flex flex-col gap-1 rounded-md border border-slate-200 bg-slate-50 p-3 text-slate-500" data-testid="owner-only">
            <div className="text-sm font-medium">Только у владельца — правом не выдаётся</div>
            {OWNER_ONLY.map((o) => (
              <div key={o.label} className="text-xs">
                {o.label} <span className="text-slate-400">— {o.why}</span>
              </div>
            ))}
          </div>
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
        title="Пользователи и права"
        subtitle="Кто входит в программу и что можно каждому администратору — «Что можно» в строке"
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
                <td className="px-2 py-2" data-testid="user-telegram" onClick={(ev) => u.role === 'admin' && u.active && ev.stopPropagation()}>
                  {/* Накладные в бот от администратора (задача 003, Н6): привязка — прямо в колонке. */}
                  {u.role === 'admin' && u.active ? (
                    <div className="flex flex-col items-start gap-0.5">
                      <button type="button" className="text-sky-700 hover:underline" data-testid="user-telegram-btn" onClick={() => setTelegram(u)}>
                        {u.telegram_linked_at ? 'привязан · изменить' : 'Привязать'}
                      </button>
                      {noReceiptsRight(u) && <span className="text-xs text-slate-500">нет права «Приход товара» — фото не примутся</span>}
                    </div>
                  ) : u.telegram_linked_at ? (
                    <Badge tone="green">привязан</Badge>
                  ) : (
                    <span className="text-slate-400">—</span>
                  )}
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
        <AdminTelegramModal
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
            <Field label="Роль" hint={form.role === 'admin' ? 'Права — по умолчанию; поменять — «Что можно» в строке' : undefined}>
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
          others={list.data ?? []}
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
