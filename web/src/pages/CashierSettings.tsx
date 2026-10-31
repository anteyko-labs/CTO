// «Настройки → Касса» (задача 003 Н2–Н4, ADR-146–{B4.3}): только владелец. Где стоит каждая кнопка
// режима кассы и в каком порядке, «Как было», готовый набор «Частое — у итога»; высота плиток и купюры
// у «Получено»; скидка на кассе и предел администратора; прибыль на бумаге и на экране чека у кассы.
// Каждое изменение — в журнал на сервере. Права на кнопки не меняются.
import { useState } from 'react'
import { Link } from 'react-router-dom'
import { Button, Card, CardTitle, ErrorBox, Field, Loading, Notice, PageHeader, toast } from '../components/ui'
import { newOpId, put } from '../lib/api'
import {
  BUTTONS,
  buttonDef,
  frequentPreset,
  layoutReset,
  loadCashierSettings,
  move,
  PLACE_LABELS,
  setPlace,
  withDefaults,
  type CashierSettings,
  type Place,
} from '../lib/cashierLayout'
import { formatSom, parseSom, somInput } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'

/** Места на образце экрана — в порядке сверху вниз. */
const PREVIEW: { place: Place; title: string }[] = [
  { place: 'bar', title: 'Полоса сверху' },
  { place: 'search', title: 'Под поиском' },
  { place: 'total', title: 'У «Итого»' },
  { place: 'pay', title: 'Под оплатой' },
  { place: 'more', title: 'Окно «Ещё действия…»' },
]

export default function CashierSettingsPage() {
  const st = useLoad(loadCashierSettings, [])
  if (!st.data) return st.error ? <ErrorBox error={st.error} /> : <Loading />
  return <CashierSettingsForm initial={st.data} />
}

function CashierSettingsForm({ initial }: { initial: CashierSettings }) {
  const act = useAction()
  const [s, setS] = useState<CashierSettings>(initial)
  // Поля ввода чисел — текстом, пока владелец печатает.
  const [quickText, setQuickText] = useState(initial.discount_quick.join(', '))
  const [pctText, setPctText] = useState(initial.admin_limit_pct === null ? '' : String(initial.admin_limit_pct))
  const [sumText, setSumText] = useState(initial.admin_limit_tyiyn === null ? '' : somInput(initial.admin_limit_tyiyn))

  const quick = quickText
    .split(/[\s,;]+/)
    .filter(Boolean)
    .map((x) => (/^\d{1,2}$/.test(x) ? Number(x) : NaN))
  const quickOk = quick.length <= 5 && quick.every((q) => Number.isInteger(q) && q >= 1 && q <= 99)
  const pct = pctText.trim() ? (/^\d{1,3}$/.test(pctText.trim()) ? Number(pctText) : NaN) : null
  const pctOk = pct === null || (Number.isInteger(pct) && pct >= 0 && pct <= 100)
  const sum = sumText.trim() ? parseSom(sumText) : null
  const sumOk = !sumText.trim() || sum !== null

  const save = () =>
    void act.run(async () => {
      const body: CashierSettings = {
        ...s,
        discount_quick: quick,
        admin_limit_pct: pct,
        admin_limit_tyiyn: sum,
      }
      const saved = await put<CashierSettings>('/settings/cashier', { op_id: newOpId(), ...body })
      setS(withDefaults(saved))
      toast('Настройки кассы сохранены')
    })

  const set = (patch: Partial<CashierSettings>) => setS({ ...s, ...patch })

  return (
    <div>
      <PageHeader
        title="Касса"
        subtitle="Кнопки режима кассы, скидка, прибыль на бумаге. Права на кнопки не меняются: без права кнопки нет нигде"
        back={<Link to="/settings">← Настройки</Link>}
        actions={
          <Button disabled={act.busy || !quickOk || !pctOk || !sumOk} onClick={save}>
            Сохранить
          </Button>
        }
      />
      <ErrorBox error={act.error} />
      <div className="space-y-4">
        <Card data-testid="cashier-layout">
          <CardTitle
            actions={
              <span className="flex flex-wrap gap-2">
                <Button variant="secondary" onClick={() => set(frequentPreset(s))}>
                  Частое — у итога
                </Button>
                <Button variant="ghost" onClick={() => set(layoutReset(s))}>
                  Как было
                </Button>
              </span>
            }
          >
            Кнопки режима кассы
          </CardTitle>
          <p className="mb-3 text-sm text-slate-500">
            «Частое — у итога»: скидка, баллы и аккумулятор в оплату — крупно у «Итого», редкие действия — в окне «Ещё действия…», плитки
            ходового выше, «Провести чек» во всю ширину. Потом можно поправить. «Как было» — раскладка до изменений.
          </p>
          <div className="grid gap-4 lg:grid-cols-[minmax(0,1fr)_18rem]">
            <ul className="divide-y divide-slate-100 rounded-md border border-slate-200">
              {s.buttons.map((b) => {
                const def = buttonDef(b.id)
                if (!def) return null
                return (
                  <li key={b.id} className="flex flex-wrap items-center gap-2 px-3 py-2" data-testid={`layout-${b.id}`}>
                    <span className="min-w-40 flex-1 font-medium">{def.label}</span>
                    <select
                      aria-label={`Место: ${def.label}`}
                      className="w-48"
                      value={b.place}
                      onChange={(e) => set({ buttons: setPlace(s.buttons, b.id, e.target.value as Place) })}
                    >
                      {def.places.map((p) => (
                        <option key={p} value={p}>
                          {PLACE_LABELS[p]}
                        </option>
                      ))}
                    </select>
                    <Button variant="ghost" className="px-3" aria-label={`Выше: ${def.label}`} onClick={() => set({ buttons: move(s.buttons, b.id, -1) })}>
                      ↑
                    </Button>
                    <Button variant="ghost" className="px-3" aria-label={`Ниже: ${def.label}`} onClick={() => set({ buttons: move(s.buttons, b.id, 1) })}>
                      ↓
                    </Button>
                  </li>
                )
              })}
            </ul>
            {/* Образец экрана: что где будет стоять (без учёта прав вошедшего). */}
            <div className="flex flex-col gap-2 rounded-md bg-slate-50 p-3 text-sm" data-testid="layout-preview">
              <div className="font-medium text-slate-600">Образец</div>
              {PREVIEW.map(({ place, title }) => {
                const ids = s.buttons.filter((b) => b.place === place).map((b) => b.id)
                return (
                  <div key={place} className={`rounded border px-2 py-1 ${place === 'bar' ? 'border-slate-700 bg-slate-800 text-white' : 'border-slate-200 bg-white'}`}>
                    <div className={`text-xs ${place === 'bar' ? 'text-slate-300' : 'text-slate-500'}`}>{title}</div>
                    {ids.length === 0 ? <span className="text-slate-400">—</span> : ids.map((id) => buttonDef(id)?.label).join(' · ')}
                  </div>
                )
              })}
              {s.buttons.some((b) => b.place === 'hidden') && (
                <div className="text-xs text-slate-500">
                  Скрыто:{' '}
                  {s.buttons
                    .filter((b) => b.place === 'hidden')
                    .map((b) => buttonDef(b.id)?.label)
                    .join(', ')}
                </div>
              )}
            </div>
          </div>
          <div className="mt-4 grid gap-3 sm:grid-cols-2">
            <label className="inline-flex items-center gap-2 text-sm">
              <input type="checkbox" className="h-4 w-4" checked={s.submit_wide} onChange={(e) => set({ submit_wide: e.target.checked })} />
              «Провести чек» во всю ширину, «Отложить» и «Очистить» — над ним
            </label>
            <label className="inline-flex items-center gap-2 text-sm">
              <input type="checkbox" className="h-4 w-4" checked={s.exit_grey} onChange={(e) => set({ exit_grey: e.target.checked })} />
              «Выйти из режима кассы» — серой кнопкой
            </label>
            <Field label="Высота плиток ходового" hint="Доля высоты экрана; было — 24 %">
              <select value={s.tiles_dvh} onChange={(e) => set({ tiles_dvh: Number(e.target.value) })}>
                {[...new Set([15, 24, 34, 44, 50, s.tiles_dvh])]
                  .sort((a, b) => a - b)
                  .map((v) => (
                    <option key={v} value={v}>
                      {v} %
                    </option>
                  ))}
              </select>
            </Field>
            <Field label="Купюр у «Получено»" hint="Кнопки быстрой суммы рядом с «Без сдачи»; было — одна">
              <select value={s.notes} onChange={(e) => set({ notes: Number(e.target.value) })}>
                {[1, 2, 3].map((v) => (
                  <option key={v} value={v}>
                    {v}
                  </option>
                ))}
              </select>
            </Field>
          </div>
          <p className="mt-2 text-xs text-slate-500">Всего кнопок: {BUTTONS.length}. Касса без связи берёт раскладку из снимка.</p>
        </Card>

        <Card data-testid="cashier-discount">
          <CardTitle>Скидка</CardTitle>
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label="Быстрые проценты" hint="Через запятую, до пяти, например 5, 10, 15" error={quickOk ? null : 'от 1 до 99, не больше пяти'}>
              <input id="discount-quick" value={quickText} onChange={(e) => setQuickText(e.target.value)} />
            </Field>
            <label className="inline-flex items-center gap-2 self-center text-sm">
              <input type="checkbox" className="h-4 w-4" checked={s.discount_services} onChange={(e) => set({ discount_services: e.target.checked })} />
              Скидка на весь чек снижает и цены услуг
            </label>
            <Field label="Предел скидки администратора, %" hint="От суммы чека по прайсу. Пусто — без предела" error={pctOk ? null : 'от 0 до 100'}>
              <input id="admin-limit-pct" inputMode="numeric" value={pctText} onChange={(e) => setPctText(e.target.value.replace(/\D/g, '').slice(0, 3))} />
            </Field>
            <Field
              label="Предел скидки администратора, сом"
              hint={sum !== null ? `Не больше ${formatSom(sum)} на чек` : 'Пусто — без предела'}
              error={sumOk ? null : 'сумма в сомах'}
            >
              <input id="admin-limit-sum" inputMode="decimal" value={sumText} onChange={(e) => setSumText(e.target.value)} />
            </Field>
            <label className="inline-flex items-center gap-2 text-sm">
              <input type="checkbox" className="h-4 w-4" checked={s.print_discount} onChange={(e) => set({ print_discount: e.target.checked })} />
              Печатать строку «Скидка» в чеке
            </label>
          </div>
          <Notice tone="sky">
            Скидку дают владелец и администраторы с правом «Скидка в чеке» (Пользователи → Что можно). Предел проверяет сервер: больше — чек не
            проведётся, без связи — проведётся, а вам придёт причина. Дешевле закупки продать нельзя.
          </Notice>
        </Card>

        <Card data-testid="cashier-profit">
          <CardTitle>Прибыль</CardTitle>
          <div className="flex flex-col gap-2">
            <label className="inline-flex items-center gap-2 text-sm">
              <input type="checkbox" className="h-4 w-4" checked={s.print_shift_profit} onChange={(e) => set({ print_shift_profit: e.target.checked })} />
              Печатать прибыль в чеке смены
            </label>
            <label className="inline-flex items-center gap-2 text-sm">
              <input type="checkbox" className="h-4 w-4" checked={s.sale_cost_on_till} onChange={(e) => set({ sale_cost_on_till: e.target.checked })} />
              Себестоимость и прибыль на экране чека на кассе (в режиме кассы)
            </label>
          </div>
          <p className="mt-2 text-sm text-slate-500">
            Себестоимость, из чего сложилась чистая прибыль и чистая — в отчёте смены в Телеграм; прибыль по каждому чеку — если чеки включены в
            «<Link to="/settings/bot-feed" className="text-sky-700 underline">Уведомления в бот</Link>». Экраны «Смена» и «Отчёт смены» у владельца — как
            раньше.
          </p>
        </Card>
      </div>
    </div>
  )
}
