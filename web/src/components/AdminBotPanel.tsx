// «Что видит в боте» и лента администратора (SPEC-40): пункты меню чата администратора по его
// правам и выключатели событий, которые владелец ему присылает. По умолчанию лента не идёт; в
// списке — только события без денег владельца (I13). Только владелец, в окне «Телеграм» из
// «Пользователей».
import { useState } from 'react'
import { Button, ErrorBox, Loading, toast } from './ui'
import { type AdminBot, type AdminBotHelper, applyAdminFeed, enabledCount, helperLimit, helperStatus, switchAdminEvent } from '../lib/adminBot'
import { get, newOpId, put } from '../lib/api'
import { useAction, useLoad } from '../lib/hooks'

/** Помощник в боте (SPEC-40 «Дополнено: помощник»): выключатель и лимит вопросов в день. */
function HelperBlock({
  userId,
  helper,
  onSaved,
  reload,
}: {
  userId: string
  helper: AdminBotHelper
  onSaved: (h: AdminBotHelper) => void
  reload: () => void
}) {
  const act = useAction()
  const [limit, setLimit] = useState(String(helper.daily_limit))
  const parsed = helperLimit(limit)
  // Выключатель меняется сразу; не сохранилось — настройка перечитывается с сервера.
  const save = (on: boolean, daily: number, done: string) => {
    onSaved({ ...helper, on, daily_limit: daily })
    void act.run(async () => {
      try {
        const saved = await put<{ on: boolean; daily_limit: number }>(`/users/${userId}/bot-helper`, { op_id: newOpId(), on, daily_limit: daily })
        onSaved({ ...helper, ...saved })
        toast(done)
      } catch (e) {
        reload()
        throw e
      }
    })
  }
  return (
    <div data-testid="admin-bot-helper">
      <div className="font-medium">Помощник в боте</div>
      <div className="text-slate-500">
        Администратор пишет своими словами («сколько осталось Mobil 5W-30», «кто должен больше всех», «запиши расход 500 на воду») — отвечает помощник, только
        по пунктам выше, которые у него есть, и только по его филиалу. Прибыли, себестоимости, сводки и денег владельца у помощника нет. Расход и старый долг —
        только черновиком с «✅ Провести». Включён у каждого входа по умолчанию — выключить можно здесь.
      </div>
      <label className="mt-1 inline-flex items-center gap-2">
        <input
          type="checkbox"
          className="h-4 w-4"
          data-testid="admin-bot-helper-on"
          checked={helper.on}
          disabled={act.busy}
          onChange={(e) =>
            save(e.target.checked, helper.daily_limit, e.target.checked ? 'Помощник в боте включён' : 'Помощник в боте выключен')
          }
        />
        <span>Включить помощника этому администратору</span>
      </label>
      <div className="mt-1 flex flex-wrap items-center gap-2">
        <label htmlFor={`helper-limit-${userId}`}>Вопросов в день, не больше</label>
        <input
          id={`helper-limit-${userId}`}
          data-testid="admin-bot-helper-limit"
          className="w-20 rounded-md border border-slate-300 px-2 py-1"
          inputMode="numeric"
          value={limit}
          onChange={(e) => setLimit(e.target.value)}
        />
        <Button
          variant="secondary"
          disabled={act.busy || parsed === null || parsed === helper.daily_limit}
          onClick={() => parsed !== null && save(helper.on, parsed, 'Лимит вопросов сохранён')}
        >
          Сохранить
        </Button>
        {parsed === null && <span className="text-rose-700">от 1 до 500</span>}
      </div>
      <div className="mt-1 text-slate-600" data-testid="admin-bot-helper-status">
        {helperStatus(helper)}
      </div>
      <ErrorBox error={act.error} />
    </div>
  )
}

export function AdminBotPanel({ userId }: { userId: string }) {
  const st = useLoad(() => get<AdminBot>(`/users/${userId}/bot`), [userId])
  const act = useAction()
  const bot = st.data

  // Выключатель меняется сразу; не сохранилось — окно перечитывает настройку с сервера.
  const save = (on: string[], done: string) => {
    st.setData((prev) => prev && applyAdminFeed(prev, on))
    void act.run(async () => {
      try {
        const saved = await put<{ on: string[] }>(`/users/${userId}/bot-feed`, { op_id: newOpId(), on })
        st.setData((prev) => prev && applyAdminFeed(prev, saved.on))
        toast(done)
      } catch (e) {
        st.reload()
        throw e
      }
    })
  }

  if (!bot) return st.error ? <ErrorBox error={st.error} /> : <Loading />
  return (
    <div className="flex flex-col gap-3" data-testid="admin-bot-panel">
      {bot.linked && bot.tg_name && (
        <div className="text-slate-600" data-testid="admin-bot-tg-name">
          Чат в Телеграме: {bot.tg_name}
        </div>
      )}
      <div>
        <div className="font-medium">Что видит в боте</div>
        <div className="text-slate-500">По его правам («Что можно»); меняются права — меняется меню. Прибыли, себестоимости и сводки в боте нет.</div>
        <ul className="mt-1 flex flex-col gap-1" data-testid="admin-bot-items">
          {bot.items.map((i) => (
            <li key={i.key} className={i.on ? '' : 'text-slate-400'} data-testid={`admin-bot-item-${i.key}`} data-on={i.on ? 'yes' : 'no'}>
              <span className="font-medium">{i.on ? '✓' : '—'} {i.button}</span> — {i.what}
              {!i.on && i.need && <span className="ml-1 text-amber-700">({i.need})</span>}
            </li>
          ))}
        </ul>
      </div>
      <HelperBlock userId={userId} helper={bot.helper} onSaved={(helper) => st.setData((prev) => prev && { ...prev, helper })} reload={st.reload} />
      <div>
        <div className="flex items-baseline justify-between gap-2">
          <span className="font-medium">События в бот</span>
          <span className="text-slate-500" data-testid="admin-bot-count">
            включено {enabledCount(bot)}
          </span>
        </div>
        <div className="text-slate-500">По умолчанию администратору ничего не приходит. Только события без денег владельца, только его филиал.</div>
        {bot.groups.map((g) => (
          <div key={g.key} className="mt-2">
            <div className="text-xs uppercase text-slate-500">{g.title}</div>
            <div className="grid gap-1 sm:grid-cols-2">
              {g.events.map((e) => (
                <label key={e.action} className="inline-flex items-center gap-2" title={e.need}>
                  <input
                    type="checkbox"
                    className="h-4 w-4"
                    data-testid={`admin-bot-event-${e.action}`}
                    checked={e.on}
                    disabled={act.busy}
                    onChange={(ev) =>
                      save(
                        switchAdminEvent(bot.on, e.action, ev.target.checked),
                        ev.target.checked ? `«${e.label}» — приходит администратору` : `«${e.label}» — не приходит администратору`,
                      )
                    }
                  />
                  <span className={e.allowed ? '' : 'text-slate-400'}>
                    {e.label}
                    {!e.allowed && ' — нет права, не придёт'}
                  </span>
                </label>
              ))}
            </div>
          </div>
        ))}
      </div>
      <ErrorBox error={act.error} />
    </div>
  )
}
