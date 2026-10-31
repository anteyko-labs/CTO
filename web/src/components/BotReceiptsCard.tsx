// «Телеграм и баллы» → «Накладные в бот присылают» (задача 003, Н6): владелец и администраторы
// филиала — привязан ли Телеграм, есть ли право «Приход товара», кнопка «Привязать»; настройка
// «Накладные в бот» — склейка фото за N минут (0 — каждое фото отдельно) и ответ на «📷 Накладная».
// Только владелец, изменение — в журнал на сервере.
import { useState } from 'react'
import { AdminTelegramModal, noReceiptsRight } from './AdminTelegramModal'
import { Badge, Button, Card, CardTitle, ErrorBox, Field, Loading, toast } from './ui'
import { get, put } from '../lib/api'
import { formatDateTime } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import type { UserRow } from '../lib/types'

export interface BotReceiptSettings {
  merge_minutes: number
  invoice_hint: string
}

/** Минуты склейки: целое от 0 до 1440; иначе — `null`. */
export function mergeMinutes(text: string): number | null {
  const t = text.trim()
  if (!/^\d{1,4}$/.test(t)) return null
  const n = Number(t)
  return n <= 1440 ? n : null
}

function Settings() {
  const st = useLoad(() => get<BotReceiptSettings>('/settings/bot-receipts'), [])
  if (!st.data) return st.loading ? <Loading /> : <ErrorBox error={st.error} />
  return <SettingsForm s={st.data} onSaved={st.reload} />
}

function SettingsForm({ s, onSaved }: { s: BotReceiptSettings; onSaved: () => void }) {
  const [minutes, setMinutes] = useState(String(s.merge_minutes))
  const [hint, setHint] = useState(s.invoice_hint)
  const save = useAction()
  const n = mergeMinutes(minutes)
  const hintOk = hint.trim().length > 0 && hint.trim().length <= 1000
  return (
    <div className="flex flex-col gap-2 rounded-md border border-slate-200 p-3">
      <div className="font-medium">Накладные в бот</div>
      <Field
        label="Склеивать фото в одну накладную, мин"
        hint="Несколько страниц одной накладной — по фото подряд. 0 — каждое фото отдельной накладной (как было); рекомендуем 10"
        error={n === null ? 'от 0 до 1440' : null}
      >
        <input id="bot-merge-minutes" className="w-24" inputMode="numeric" value={minutes} onChange={(e) => setMinutes(e.target.value)} />
      </Field>
      <Field label="Ответ на «📷 Накладная»" hint="Камеру кнопка открыть не может — так в Телеграме; ответ подсказывает, как снять" error={hintOk ? null : 'от 1 до 1000 знаков'}>
        <textarea id="bot-invoice-hint" rows={2} value={hint} onChange={(e) => setHint(e.target.value)} />
      </Field>
      <ErrorBox error={save.error} />
      <div>
        <Button
          disabled={n === null || !hintOk || save.busy}
          onClick={() =>
            void save.run(async () => {
              await put('/settings/bot-receipts', { merge_minutes: n, invoice_hint: hint.trim() })
              toast('Сохранено')
              onSaved()
            })
          }
        >
          Сохранить
        </Button>
      </div>
    </div>
  )
}

export function BotReceiptsCard({ ownerLinked }: { ownerLinked: boolean }) {
  const users = useLoad(() => get<UserRow[]>('/users'), [])
  const [linking, setLinking] = useState<UserRow | null>(null)
  const admins = (users.data ?? []).filter((u) => u.role === 'admin' && u.active)
  return (
    <Card className="flex flex-col gap-3 text-sm" data-testid="bot-receipts-card">
      <CardTitle>Накладные в бот присылают</CardTitle>
      <div className="-mt-2 text-slate-600">
        Фото накладной из привязанного чата становится черновиком прихода («Приход» → «Накладные по фото»). Администратору Телеграм привязывает владелец — кнопкой
        «Привязать» здесь или в «Пользователях».
      </div>
      <ErrorBox error={users.error} />
      <ul className="flex flex-col divide-y divide-slate-100 rounded-md border border-slate-200">
        <li className="flex flex-wrap items-center justify-between gap-2 px-3 py-2">
          <span>Владелец</span>
          {ownerLinked ? <Badge tone="green">привязан</Badge> : <span className="text-slate-500">не привязан — карточка «Привязать Телеграм» выше</span>}
        </li>
        {admins.map((u) => (
          <li key={u.id} className="flex flex-wrap items-center justify-between gap-2 px-3 py-2" data-testid="bot-receipts-admin">
            <div className="flex flex-col">
              <span>
                {u.full_name} <span className="text-slate-500">({u.login})</span>
              </span>
              <span className="text-xs text-slate-500">
                {u.telegram_linked_at ? `привязан с ${formatDateTime(u.telegram_linked_at)}` : 'Телеграм не привязан'}
                {noReceiptsRight(u) ? ' · нет права «Приход товара» — фото не примутся' : ' · право «Приход товара» есть'}
              </span>
            </div>
            <Button variant="secondary" className="min-h-[34px] px-3 py-1 text-xs" onClick={() => setLinking(u)}>
              {u.telegram_linked_at ? 'Изменить' : 'Привязать'}
            </Button>
          </li>
        ))}
        {users.data && admins.length === 0 && <li className="px-3 py-2 text-slate-500">Администраторов нет — их заводят в «Пользователях».</li>}
      </ul>
      <Settings />
      {linking && (
        <AdminTelegramModal
          user={linking}
          onClose={() => setLinking(null)}
          onChanged={() => {
            setLinking(null)
            users.reload()
          }}
        />
      )}
    </Card>
  )
}
