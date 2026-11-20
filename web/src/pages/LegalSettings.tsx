// «Настройки → Юрлица» (задача 039 C, ADR-202): подтверждение заправки фирмой при чеке в долг.
// У каждого филиала своя; правит владелец, изменение — в журнал.
import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { Button, Card, CardTitle, ErrorBox, Field, Loading, PageHeader, toast } from '../components/ui'
import { get, newOpId, put } from '../lib/api'
import { useAction, useLoad } from '../lib/hooks'

type ApprovalMode = 'wait' | 'after' | 'timeout' | 'off'
interface LegalSettings {
  approval_mode: ApprovalMode
  approval_timeout_min: number
}

const MODES: { value: ApprovalMode; title: string; text: string }[] = [
  { value: 'timeout', title: 'Ждать с таймаутом', text: 'Чек ждёт ответа фирмы; не ответили за N минут — кассир может провести без подтверждения' },
  { value: 'wait', title: 'Ждать', text: 'Чек не проводится, пока фирма не подтвердит' },
  { value: 'after', title: 'После', text: 'Чек проводится сразу в долг, подтверждение — отметка на чеке; отказ — вам событие' },
  { value: 'off', title: 'Не спрашивать', text: 'Как раньше: фирме запрос не уходит' },
]

export default function LegalSettingsPage() {
  const data = useLoad(() => get<LegalSettings>('/settings/legal'), [])
  const [form, setForm] = useState<LegalSettings | null>(null)
  const { run, busy, error } = useAction()
  useEffect(() => {
    if (data.data) setForm(data.data)
  }, [data.data])
  const save = () =>
    run(async () => {
      if (!form) return
      const s = await put<LegalSettings>('/settings/legal', { op_id: newOpId(), ...form })
      setForm(s)
      toast('Сохранено')
    })
  return (
    <div className="flex flex-col gap-4">
      <PageHeader title="Юрлица" back={<Link to="/settings">← Настройки</Link>} />
      <ErrorBox error={data.error ?? error} />
      {!form ? (
        <Loading />
      ) : (
        <Card className="flex flex-col gap-3">
          <CardTitle>Подтверждение заправки фирмой</CardTitle>
          <p className="text-sm text-slate-600">
            Чек в долг фирмы — запрос «Подтвердите обслуживание» в Телеграм фирмы и в её кабинет. Фирма без Телеграма и кабинета — чек как раньше, отметка «не спрошено».
          </p>
          <div className="flex flex-col gap-2" role="radiogroup">
            {MODES.map((m) => (
              <label key={m.value} className="flex cursor-pointer items-start gap-2 rounded-md border border-slate-200 p-2">
                <input type="radio" name="approval_mode" checked={form.approval_mode === m.value} onChange={() => setForm({ ...form, approval_mode: m.value })} />
                <span>
                  <span className="font-medium">{m.title}</span>
                  <span className="block text-sm text-slate-600">{m.text}</span>
                </span>
              </label>
            ))}
          </div>
          {form.approval_mode === 'timeout' && (
            <Field label="Таймаут, минут">
              <input
                type="number"
                min={1}
                max={240}
                value={form.approval_timeout_min}
                onChange={(e) => setForm({ ...form, approval_timeout_min: Number(e.target.value) || 1 })}
              />
            </Field>
          )}
          <div>
            <Button disabled={busy} onClick={() => void save()}>
              Сохранить
            </Button>
          </div>
        </Card>
      )}
    </div>
  )
}
