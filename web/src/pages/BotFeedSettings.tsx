// «Настройки → Уведомления в бот» (задача 002 П11, ADR-138): что приходит этому владельцу в
// Телеграм. У каждого владельца своя настройка; экран «Уведомления» в программе показывает всё.
import { Link } from 'react-router-dom'
import { Card, CardTitle, ErrorBox, Loading, Notice, PageHeader, toast } from '../components/ui'
import { get, newOpId, put } from '../lib/api'
import { applyFeed, type BotFeed, groupCount, SALES_MODES, type SalesMode, switchEvent } from '../lib/botFeed'
import { useAction, useLoad } from '../lib/hooks'

export default function BotFeedSettings() {
  const st = useLoad(() => get<BotFeed>('/settings/bot-feed'), [])
  const act = useAction()
  const s = st.data

  // Выключатель меняется сразу; не сохранилось — экран перечитывает настройку с сервера.
  const save = (off: string[], sales: SalesMode, done: string, profit: boolean = s?.profit ?? true) => {
    st.setData((prev) => prev && applyFeed(prev, off, sales, profit))
    void act.run(async () => {
      try {
        const saved = await put<{ off: string[]; sales: SalesMode; profit?: boolean }>('/settings/bot-feed', { op_id: newOpId(), off, sales, profit })
        st.setData((prev) => prev && applyFeed(prev, saved.off, saved.sales, saved.profit ?? true))
        toast(done)
      } catch (e) {
        st.reload()
        throw e
      }
    })
  }

  return (
    <div>
      <PageHeader
        title="Уведомления в бот"
        subtitle="Что приходит вам в Телеграм. Экран «Уведомления» в программе показывает всё, как раньше"
        back={<Link to="/settings">← Настройки</Link>}
      />
      <ErrorBox error={st.error ?? act.error} />
      {!s ? (
        <Loading />
      ) : (
        <div className="space-y-4">
          <Card data-testid="bot-feed-sales">
            <CardTitle>Чеки и возвраты</CardTitle>
            <div className="flex flex-col gap-2" role="radiogroup" aria-label="Чеки и возвраты">
              {SALES_MODES.map((m) => (
                <label key={m.value} className="inline-flex items-center gap-2 text-sm">
                  <input
                    type="radio"
                    name="bot-feed-sales"
                    className="h-4 w-4"
                    checked={s.sales === m.value}
                    disabled={act.busy}
                    onChange={() => save(s.off, m.value, `Чеки: ${m.label.toLowerCase()}`)}
                  />
                  {m.label}
                </label>
              ))}
            </div>
            {/* Прибыль — не на бумаге, а в Телеграм (задача 003 Н4): в сообщении о чеке — себестоимость и прибыль. */}
            <label className="mt-3 inline-flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                className="h-4 w-4"
                checked={s.profit ?? true}
                disabled={act.busy}
                onChange={(e) =>
                  save(s.off, s.sales, e.target.checked ? 'Прибыль в сообщениях о чеках — приходит' : 'Прибыль в сообщениях о чеках — не приходит', e.target.checked)
                }
              />
              Себестоимость и прибыль в сообщениях о чеках
            </label>
            <p className="mt-2 text-sm text-slate-500">
              Отчёт при закрытии смены (с себестоимостью и из чего сложилась чистая прибыль) и «Итог дня» приходят всегда. Чек смены на
              бумаге прибыль не печатает — «Настройки → Касса».
            </p>
          </Card>
          {s.groups.map((g) => {
            const n = groupCount(g)
            return (
              <Card key={g.key} data-testid={`bot-feed-group-${g.key}`}>
                <CardTitle
                  actions={
                    <span className="text-sm text-slate-500">
                      включено {n.on} из {n.all}
                    </span>
                  }
                >
                  {g.title}
                </CardTitle>
                <div className="grid gap-2 sm:grid-cols-2">
                  {g.events.map((e) => (
                    <label key={e.action} className="inline-flex items-center gap-2 text-sm">
                      <input
                        type="checkbox"
                        className="h-4 w-4"
                        checked={e.on}
                        disabled={act.busy}
                        onChange={(ev) =>
                          save(
                            switchEvent(s.off, e.action, ev.target.checked),
                            s.sales,
                            ev.target.checked ? `«${e.label}» — приходит в бот` : `«${e.label}» — не приходит в бот`,
                          )
                        }
                      />
                      {e.label}
                    </label>
                  ))}
                </div>
              </Card>
            )
          })}
          <Notice tone="sky">Настройка только ваша: у другого владельца с ботом — своя.</Notice>
        </div>
      )}
    </div>
  )
}
