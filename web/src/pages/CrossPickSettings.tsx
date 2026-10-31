// «Настройки → Кросс-номера» (задача 002 П10, ADR-145): как подбирать кросс-номера фильтров
// сервисом разбора — записывать сразу или предлагать — и сколько подборов в день. Правит владелец,
// изменение — в журнал.
import { useState } from 'react'
import { Link } from 'react-router-dom'
import { Button, Card, CardTitle, ErrorBox, Field, Loading, Notice, PageHeader, toast } from '../components/ui'
import { get, newOpId, put } from '../lib/api'
import { CROSS_MODES, type CrossMode, type CrossPickSettings } from '../lib/crossPick'
import { useAction, useLoad } from '../lib/hooks'

export default function CrossPickSettingsPage() {
  const st = useLoad(() => get<CrossPickSettings>('/settings/cross-pick'), [])
  const act = useAction()
  const s = st.data
  const [limit, setLimit] = useState<string | null>(null)

  const save = (mode: CrossMode, daily_limit: number, done: string) =>
    void act.run(async () => {
      try {
        const saved = await put<{ mode: CrossMode; daily_limit: number }>('/settings/cross-pick', { op_id: newOpId(), mode, daily_limit })
        st.setData((prev) => prev && { ...prev, ...saved })
        setLimit(null)
        toast(done)
      } catch (e) {
        st.reload()
        throw e
      }
    })

  const limitText = limit ?? (s ? String(s.daily_limit) : '')
  const limitValid = /^\d+$/.test(limitText) && Number(limitText) <= 1000

  return (
    <div>
      <PageHeader
        title="Кросс-номера"
        subtitle="Подбор номеров аналогов фильтров сервисом разбора: в карточке товара и для всей категории"
        back={<Link to="/settings">← Настройки</Link>}
      />
      <ErrorBox error={st.error ?? act.error} />
      {!s ? (
        <Loading />
      ) : (
        <div className="space-y-4">
          {!s.available && <Notice>Сервис разбора на сервере не настроен (нет ключа) — кнопка «Подобрать кросс-номера» не работает.</Notice>}
          <Card data-testid="cross-pick-mode">
            <CardTitle>Что делать с подобранными номерами</CardTitle>
            <div className="flex flex-col gap-2" role="radiogroup" aria-label="Подобранные номера">
              {CROSS_MODES.map((m) => (
                <label key={m.value} className="inline-flex items-start gap-2 text-sm">
                  <input
                    type="radio"
                    name="cross-pick-mode"
                    className="mt-0.5 h-4 w-4"
                    checked={s.mode === m.value}
                    disabled={act.busy}
                    onChange={() => {
                      st.setData({ ...s, mode: m.value })
                      save(m.value, s.daily_limit, `Подобранные номера: ${m.label.toLowerCase()}`)
                    }}
                  />
                  <span>
                    <b className="font-medium">{m.label}</b> — {m.hint}
                  </span>
                </label>
              ))}
            </div>
            <p className="mt-2 text-sm text-slate-500">Каждая запись — в журнале действий с пометкой «подобрано автоматически».</p>
          </Card>
          <Card data-testid="cross-pick-limit">
            <CardTitle actions={<span className="text-sm text-slate-500">сегодня подобрано {s.used_today}</span>}>Сколько подборов в день</CardTitle>
            <div className="flex flex-wrap items-end gap-3">
              <Field label="Подборов в день на филиал" hint="Один товар — один подбор. 0 — подбор выключен">
                <input id="cross-pick-limit" inputMode="numeric" value={limitText} onChange={(e) => setLimit(e.target.value.replace(/\D/g, ''))} />
              </Field>
              <Button disabled={act.busy || !limitValid || limit === null} onClick={() => save(s.mode, Number(limitText), `Подборов в день: ${limitText}`)}>
                Сохранить
              </Button>
            </div>
          </Card>
        </div>
      )}
    </div>
  )
}
