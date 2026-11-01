// «Что видит в боте» и лента администратора (SPEC-40): пункты меню чата администратора по его
// правам и выключатели событий, которые владелец ему присылает. По умолчанию лента не идёт; в
// списке — только события без денег владельца (I13). Только владелец, в окне «Телеграм» из
// «Пользователей».
import { ErrorBox, Loading, toast } from './ui'
import { type AdminBot, applyAdminFeed, enabledCount, switchAdminEvent } from '../lib/adminBot'
import { get, newOpId, put } from '../lib/api'
import { useAction, useLoad } from '../lib/hooks'

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
