// «Настройки → Терминал Face ID» (задача 039 A, ADR-200): адрес приёма событий филиала для Dahua
// («Сеть → Загрузка событий (HTTP)»), «Сменить ключ», окно подтверждения в минутах, последние события.
import { useState } from 'react'
import { Link } from 'react-router-dom'
import { Button, Card, CardTitle, ErrorBox, Field, Loading, PageHeader, Table, toast } from '../components/ui'
import { get, put } from '../lib/api'
import { OUTCOME_LABEL, type FaceIdSettings as Settings } from '../lib/faceid'
import { useAction, useLoad } from '../lib/hooks'

export default function FaceIdSettings() {
  const st = useLoad(() => get<Settings>('/settings/faceid'), [])
  const [windowMin, setWindowMin] = useState<string | null>(null)
  const act = useAction()
  if (st.loading && !st.data) return <Loading />
  if (!st.data) return <ErrorBox error={st.error} />
  const s = st.data
  const url = `${window.location.origin}${s.path}`
  const w = windowMin ?? String(s.window_min)
  const wOk = /^\d+$/.test(w) && Number(w) >= 1 && Number(w) <= 30
  const save = (body: { window_min?: number; new_key?: boolean }, text: string) =>
    void act.run(async () => {
      await put<Settings>('/settings/faceid', body)
      setWindowMin(null)
      st.reload()
      toast(text)
    })
  return (
    <div className="flex max-w-3xl flex-col gap-4">
      <PageHeader
        title="Терминал Face ID"
        subtitle="Рабочий день: кнопка в боте у работника и лицо на терминале Dahua"
        actions={
          <Link to="/timesheet" className="text-sm text-sky-700 hover:underline">
            Табель →
          </Link>
        }
      />
      <Card className="flex flex-col gap-3">
        <CardTitle>Адрес приёма событий</CardTitle>
        <p className="text-sm text-slate-600">
          Впишите этот адрес в терминале Dahua: «Сеть → Загрузка событий (HTTP)». Номер сотрудника в терминале (UserID) задаётся в его
          карточке в «Сотрудниках».
        </p>
        <code data-testid="faceid-url" className="break-all rounded bg-slate-100 px-2 py-1 text-sm">
          {url}
        </code>
        <div className="flex flex-wrap gap-2">
          <Button
            variant="secondary"
            onClick={() => void navigator.clipboard?.writeText(url).then(() => toast('Адрес скопирован'))}
          >
            Скопировать
          </Button>
          <Button
            variant="secondary"
            disabled={act.busy}
            onClick={() => {
              if (window.confirm('Сменить ключ? Старый адрес перестанет работать — впишите новый в терминал.')) save({ new_key: true }, 'Ключ сменён')
            }}
          >
            Сменить ключ
          </Button>
        </div>
      </Card>
      <Card className="flex flex-col gap-3">
        <CardTitle>Окно подтверждения</CardTitle>
        <div className="flex flex-wrap items-end gap-2">
          <Field label="Минут после нажатия кнопки" hint="За это время работник подходит к терминалу" error={wOk ? undefined : 'От 1 до 30'}>
            <input id="faceid-window" inputMode="numeric" className="w-32" value={w} onChange={(e) => setWindowMin(e.target.value)} />
          </Field>
          <Button disabled={act.busy || !wOk || Number(w) === s.window_min} onClick={() => save({ window_min: Number(w) }, 'Окно сохранено')}>
            Сохранить
          </Button>
        </div>
      </Card>
      <ErrorBox error={act.error} />
      <Card className="flex flex-col gap-3">
        <CardTitle>Последние события терминала</CardTitle>
        {s.events.length === 0 ? (
          <p className="text-sm text-slate-500">Событий пока не было.</p>
        ) : (
          <Table head={['Когда', 'Номер', 'Сотрудник', 'Итог']}>
            {s.events.map((e, i) => (
              <tr key={i}>
                <td>{new Date(e.created_at).toLocaleString('ru-RU', { timeZone: 'Asia/Bishkek' })}</td>
                <td>{e.user_id ?? '—'}</td>
                <td>{e.employee ?? '—'}</td>
                <td>{OUTCOME_LABEL[e.outcome] ?? e.outcome}</td>
              </tr>
            ))}
          </Table>
        )}
      </Card>
    </div>
  )
}
