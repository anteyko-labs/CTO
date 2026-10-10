// Экран входа владельца или администратора (SPEC-01).
import { useState, type FormEvent } from 'react'
import { Button, ErrorBox, Field } from '../components/ui'
import { useAuth } from '../lib/auth'
import { useAction } from '../lib/hooks'

export default function Login() {
  const { login } = useAuth()
  const [form, setForm] = useState({ login: '', password: '' })
  const [show, setShow] = useState(false)
  const { busy, error, run } = useAction()

  const submit = (e: FormEvent) => {
    e.preventDefault()
    void run(() => login(form.login, form.password))
  }

  return (
    <div className="flex min-h-screen items-center justify-center p-4">
      <form onSubmit={submit} className="flex w-full max-w-sm flex-col gap-4 rounded-lg bg-white p-6 shadow">
        <div className="text-center">
          <h1 className="text-2xl font-bold">Avtodom</h1>
          <div className="mt-1 text-sm text-slate-500">Точка Avtodom · вход для сотрудников</div>
        </div>
        <Field label="Логин">
          <input autoFocus autoComplete="username" value={form.login} onChange={(e) => setForm({ ...form, login: e.target.value })} />
        </Field>
        <Field label="Пароль">
          <div className="relative">
            <input
              type={show ? 'text' : 'password'}
              autoComplete="current-password"
              className="w-full pr-11"
              value={form.password}
              onChange={(e) => setForm({ ...form, password: e.target.value })}
            />
            {/* Подпись без слова «пароль»: поле ищется по подписи «Пароль». */}
            <button
              type="button"
              aria-label={show ? 'Скрыть набранное' : 'Показать набранное'}
              aria-pressed={show}
              title={show ? 'Скрыть' : 'Показать'}
              className="absolute inset-y-0 right-0 flex w-11 items-center justify-center text-slate-400 hover:text-slate-700"
              onClick={() => setShow((v) => !v)}
            >
              <svg viewBox="0 0 24 24" className="h-5 w-5" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z" />
                <circle cx="12" cy="12" r="3" />
                {show && <path d="M4 4l16 16" />}
              </svg>
            </button>
          </div>
        </Field>
        <ErrorBox error={error} />
        <Button type="submit" disabled={busy || !form.login || !form.password}>
          Войти
        </Button>
      </form>
    </div>
  )
}
