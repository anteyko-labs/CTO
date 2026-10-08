// Экран входа владельца или администратора (SPEC-01).
import { useState, type FormEvent } from 'react'
import { Button, ErrorBox, Field } from '../components/ui'
import { useAuth } from '../lib/auth'
import { useAction } from '../lib/hooks'

export default function Login() {
  const { login } = useAuth()
  const [form, setForm] = useState({ login: '', password: '' })
  const { busy, error, run } = useAction()

  const submit = (e: FormEvent) => {
    e.preventDefault()
    void run(() => login(form.login, form.password))
  }

  return (
    <div className="flex min-h-screen items-center justify-center p-4">
      <form onSubmit={submit} className="flex w-full max-w-sm flex-col gap-4 rounded-lg bg-white p-6 shadow">
        <h1 className="text-center text-2xl font-bold">Avtodom</h1>
        <Field label="Логин">
          <input autoFocus autoComplete="username" value={form.login} onChange={(e) => setForm({ ...form, login: e.target.value })} />
        </Field>
        <Field label="Пароль">
          <input
            type="password"
            autoComplete="current-password"
            value={form.password}
            onChange={(e) => setForm({ ...form, password: e.target.value })}
          />
        </Field>
        <ErrorBox error={error} />
        <Button type="submit" disabled={busy || !form.login || !form.password}>
          Войти
        </Button>
      </form>
    </div>
  )
}
