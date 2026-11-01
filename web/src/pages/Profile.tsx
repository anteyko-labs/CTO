// «Профиль» (задача 005 Н1, ADR-170): кто вошёл и «Сменить свой пароль» — у любого входа
// (владелец, администратор, личный вход сотрудника). Прежний пароль, новый не короче 8 символов,
// op_id; прочие сессии этого входа закрываются, эта остаётся (сервер, `POST /auth/password`).
import { useState } from 'react'
import { Link } from 'react-router-dom'
import { Button, Card, CardTitle, ErrorBox, Field, Missing, PageHeader, toast } from '../components/ui'
import { newOpId, post } from '../lib/api'
import { useUser } from '../lib/auth'
import { missingWithFocus } from '../lib/forms'
import { useAction } from '../lib/hooks'

const MIN_PASSWORD = 8

export default function Profile() {
  const user = useUser()
  const [form, setForm] = useState({ old: '', next: '', again: '' })
  // Своя операция на каждую смену: повтор после потерянного ответа — прежний ответ.
  const [opId, setOpId] = useState(newOpId)
  const act = useAction()
  const missing = missingWithFocus(
    [Boolean(form.old), 'текущий пароль', '#profile-old'],
    [[...form.next].length >= MIN_PASSWORD, `новый пароль не короче ${MIN_PASSWORD} символов`, '#profile-new'],
    [form.again === form.next, 'новый пароль ещё раз — так же', '#profile-again'],
  )

  const submit = () =>
    void act.run(async () => {
      const r = await post<{ changed: boolean; sessions_closed: number }>('/auth/password', {
        op_id: opId,
        old_password: form.old,
        new_password: form.next,
      })
      setForm({ old: '', next: '', again: '' })
      setOpId(newOpId())
      toast(r.sessions_closed > 0 ? `Пароль изменён, на других устройствах вход закрыт (${r.sessions_closed})` : 'Пароль изменён')
    })

  return (
    <div>
      <PageHeader title="Профиль" subtitle={`${user.full_name} · ${user.login}`} />
      <div className="flex max-w-xl flex-col gap-4">
        {user.employee_id && (
          <Card data-testid="profile-employee">
            <CardTitle>Сотрудник</CardTitle>
            <p className="text-sm text-slate-600">
              Вход связан с карточкой «{user.employee_name}». Сколько вы заработали и сколько вам должны —{' '}
              <Link to="/my-earnings" className="text-sky-700 underline">
                «Мой заработок»
              </Link>
              .
            </p>
          </Card>
        )}
        <Card data-testid="profile-password">
          <CardTitle>Сменить свой пароль</CardTitle>
          <form
            className="flex flex-col gap-3"
            onSubmit={(e) => {
              e.preventDefault()
              if (missing.length === 0) submit()
            }}
          >
            <Field label="Текущий пароль" required>
              <input id="profile-old" type="password" autoComplete="current-password" value={form.old} onChange={(e) => setForm({ ...form, old: e.target.value })} />
            </Field>
            <Field label="Новый пароль" required hint={`Не короче ${MIN_PASSWORD} символов`}>
              <input id="profile-new" type="password" autoComplete="new-password" value={form.next} onChange={(e) => setForm({ ...form, next: e.target.value })} />
            </Field>
            <Field label="Новый пароль ещё раз" required>
              <input
                id="profile-again"
                type="password"
                autoComplete="new-password"
                value={form.again}
                onChange={(e) => setForm({ ...form, again: e.target.value })}
              />
            </Field>
            <p className="text-xs text-slate-500">На других устройствах этим логином нужно будет войти заново; здесь вход остаётся.</p>
            <ErrorBox error={act.error} />
            <div className="flex flex-wrap items-center justify-end gap-2">
              <Missing items={missing} className="mr-auto" />
              <Button type="submit" disabled={act.busy || missing.length > 0}>
                Сменить пароль
              </Button>
            </div>
          </form>
        </Card>
      </div>
    </div>
  )
}
