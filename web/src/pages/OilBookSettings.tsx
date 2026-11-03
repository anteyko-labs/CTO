// «Настройки → Масляная книжка и печать» (задача 003 Н5, ADR-144): что касса печатает сама после
// чека «В сервис», что идёт в чек книжки и в товарный чек, как пишется масло канистрой и разливом,
// идут ли фильтры на срок масла. У каждого филиала своя; правит владелец, изменение — в журнал. Вид
// книжки один — нынешний (задача 002 П12): здесь меняется только содержимое.
import { useState } from 'react'
import { Link } from 'react-router-dom'
import { Button, Card, CardTitle, ErrorBox, Loading, Notice, PageHeader, toast } from '../components/ui'
import { ApiError, get, newOpId, patch, put } from '../lib/api'
import { useAction, useLoad } from '../lib/hooks'
import type { CustomSlot } from '../lib/oilBook'
import { oilBookSettingsOf, type OilBookSettings, type QtyStyle } from '../lib/oilBookSettings'

const SWITCHES: { key: Exclude<keyof OilBookSettings, 'qty_style'>; title: string; on: string; off: string; group: 'print' | 'book' }[] = [
  { key: 'auto_book', group: 'print', title: 'Чек книжки после чека «В сервис» — печатать сам', on: 'Чек книжки печатается сам', off: 'Чек книжки — только кнопкой «Печать книжки»' },
  {
    key: 'offline_book',
    group: 'print',
    title: 'Чек «В сервис» без сети — печатать книжку после отправки на сервер',
    on: 'Книжка печатается сама, когда чек ушёл на сервер',
    off: 'Книжка — кнопкой «Печать книжки» в уведомлении',
  },
  {
    key: 'book_station',
    group: 'print',
    title: 'Книжку печатать на втором принтере (станция книжки на моноблоке)',
    on: 'Книжка уходит на принтер книжки; станции нет — печать здесь',
    off: 'Книжка печатается здесь, на принтер этого окна',
  },
  {
    key: 'auto_receipt_all',
    group: 'print',
    title: 'Товарный чек после любого чека — печатать сам',
    on: 'Товарный чек печатается после каждого чека, и без сети тоже',
    off: 'Товарный чек сам — только по настройке ниже',
  },
  {
    key: 'qr_sticker',
    group: 'print',
    title: 'QR-наклейка на Телеграм-бот после чека книжки — печатать отдельно',
    on: 'После книжки — своей печатью наклейка с QR',
    off: 'QR — только кнопкой «Печать QR»',
  },
  { key: 'auto_receipt', group: 'print', title: 'Товарный чек после чека «В сервис» — печатать сам', on: 'Товарный чек печатается сам', off: 'Товарный чек — только кнопкой «Печать чека»' },
  { key: 'receipt_block', group: 'print', title: 'Блок «Масляная книжка» в товарном чеке', on: 'В товарном чеке — блок книжки', off: 'В товарном чеке — без блока книжки' },
  { key: 'with_filters', group: 'book', title: 'В чеке книжки — и фильтры (масляный, воздушный, салонный, топливный) под видом замены', on: 'В чеке книжки — масла, жидкости и фильтры', off: 'В чеке книжки — только масла и жидкости' },
  { key: 'print_filters_only', group: 'book', title: 'Фильтры в чеке книжки выключены, а заменили только фильтры — печатать чек книжки с ними', on: 'Чек только с фильтрами — печатается', off: 'Чек только с фильтрами — без чека книжки' },
  {
    key: 'filters_follow_oil',
    group: 'book',
    title: 'Масляный, воздушный и салонный фильтр с заменой масла — на пробег масла',
    on: 'Фильтры с маслом — на пробег масла',
    off: 'Каждый фильтр — по своему интервалу',
  },
]

const QTY: { value: QtyStyle; label: string; sample: string }[] = [
  { value: 'total', label: 'Итог в литрах', sample: 'Mobil 1 ESP 5W-30 · 4,5 л' },
  { value: 'split', label: 'Итог и разбивка', sample: 'Mobil 1 ESP 5W-30 · 4,5 л (1 кан. × 4 л + 0,5 л)' },
]

export default function OilBookSettingsPage() {
  const st = useLoad(() => get<OilBookSettings>('/settings/oil-book').then(oilBookSettingsOf), [])
  const act = useAction()
  const s = st.data

  // Переключатель меняется сразу; не сохранилось — экран перечитывает настройку с сервера.
  const save = (next: OilBookSettings, done: string) => {
    st.setData(next)
    void act.run(async () => {
      try {
        st.setData(await put<OilBookSettings>('/settings/oil-book', { op_id: newOpId(), ...next }))
        toast(done)
      } catch (e) {
        st.reload()
        throw e
      }
    })
  }

  const group = (g: 'print' | 'book') =>
    s &&
    SWITCHES.filter((x) => x.group === g).map((x) => (
      <label key={x.key} className="flex items-start gap-2 text-sm">
        <input
          type="checkbox"
          className="mt-0.5 h-4 w-4"
          data-testid={`oil-book-${x.key}`}
          checked={s[x.key]}
          disabled={act.busy}
          onChange={(e) => save({ ...s, [x.key]: e.target.checked }, e.target.checked ? x.on : x.off)}
        />
        <span>{x.title}</span>
      </label>
    ))

  return (
    <div>
      <PageHeader
        title="Масляная книжка и печать"
        subtitle="Что касса печатает после чека «В сервис» и что пишется в книжку. Своя у каждого филиала"
        back={<Link to="/settings">← Настройки</Link>}
      />
      <ErrorBox error={st.error ?? act.error} />
      {!s ? (
        <Loading />
      ) : (
        <div className="space-y-4">
          <Card data-testid="oil-book-print">
            <CardTitle>После чека «В сервис»</CardTitle>
            <div className="flex flex-col gap-2">{group('print')}</div>
            <p className="mt-2 text-sm text-slate-500">
              Запись в книжке после чека видна на экране кассы всегда. «Печать книжки» есть и в открытом чеке (Чеки → чек).
            </p>
          </Card>
          <Card data-testid="oil-book-content">
            <CardTitle>Что пишется в книжку и в чек книжки</CardTitle>
            <div className="flex flex-col gap-2">{group('book')}</div>
          </Card>
          <Card data-testid="oil-book-qty">
            <CardTitle>Масло канистрой и разливом одного товара — одной строкой</CardTitle>
            <div className="flex flex-col gap-2" role="radiogroup" aria-label="Масло канистрой и разливом">
              {QTY.map((q) => (
                <label key={q.value} className="inline-flex items-center gap-2 text-sm">
                  <input
                    type="radio"
                    name="oil-book-qty"
                    className="h-4 w-4"
                    checked={s.qty_style === q.value}
                    disabled={act.busy}
                    onChange={() => save({ ...s, qty_style: q.value }, `Масло в книжке: ${q.label.toLowerCase()}`)}
                  />
                  {q.label}: <span className="text-slate-500">«{q.sample}»</span>
                </label>
              ))}
            </div>
            <p className="mt-2 text-sm text-slate-500">Касается новых чеков; прежние записи книжки не меняются.</p>
          </Card>
          <CustomSlotsCard />
          <Notice tone="sky">
            Следующую замену касса заполняет сама: пробег сейчас + интервал. Если кассир её не прислал, сервер ставит ту же — в чеке, книжке и
            боте одно число.
          </Notice>
        </div>
      )}
    </div>
  )
}

type SlotDraft = { name: string; km: string; days: string }

const draftOf = (c: CustomSlot): SlotDraft => ({
  name: c.name,
  km: c.interval_km !== null ? String(c.interval_km) : '',
  days: c.interval_days !== null ? String(c.interval_days) : '',
})

const numOk = (v: string) => v.trim() === '' || /^\d+$/.test(v.trim())

/** Свои строки книжки точки (задача 016): переименовать, интервал, скрыть или показать.
 *  Создают их из окна «Запись книжки»; встроенные строки не меняются. */
function CustomSlotsCard() {
  const list = useLoad(() => get<CustomSlot[]>('/book-slots'), [])
  const act = useAction()
  const [edit, setEdit] = useState<{ id: string } & SlotDraft | null>(null)

  const send = (id: string, body: Record<string, unknown>, done: string) =>
    void act.run(async () => {
      try {
        const c = await patch<CustomSlot>(`/book-slots/${id}`, body)
        list.setData((list.data ?? []).map((x) => (x.id === c.id ? c : x)))
        setEdit(null)
        toast(done)
      } catch (e) {
        if (e instanceof ApiError && e.status === 409) throw new Error('Строка с таким названием уже есть')
        if (e instanceof ApiError && (e.status === 400 || e.status === 422)) throw new Error('Название — от 2 до 40 символов; интервал — от 500 до 200 000 км и от 1 до 1095 дней')
        throw e
      }
    })

  const slots = list.data ?? []
  return (
    <Card data-testid="oil-book-custom-slots">
      <CardTitle>Свои строки книжки</CardTitle>
      <ErrorBox error={list.error ?? act.error} />
      {list.loading && !list.data ? (
        <Loading />
      ) : slots.length === 0 ? (
        <p className="text-sm text-slate-500">Своих строк пока нет. Их создают в окне «Запись книжки»: «Что меняли» → «+ Новая строка…».</p>
      ) : (
        <ul className="divide-y divide-slate-100 text-sm">
          {slots.map((c) =>
            edit?.id === c.id ? (
              <li key={c.id} className="flex flex-wrap items-end gap-2 py-2">
                <label className="flex flex-col text-xs text-slate-600">
                  Название
                  <input value={edit.name} maxLength={40} onChange={(e) => setEdit({ ...edit, name: e.target.value })} />
                </label>
                <label className="flex w-28 flex-col text-xs text-slate-600">
                  Интервал, км
                  <input inputMode="numeric" value={edit.km} onChange={(e) => setEdit({ ...edit, km: e.target.value })} />
                </label>
                <label className="flex w-28 flex-col text-xs text-slate-600">
                  Интервал, дней
                  <input inputMode="numeric" value={edit.days} onChange={(e) => setEdit({ ...edit, days: e.target.value })} />
                </label>
                <Button variant="secondary" className="px-2 py-1 text-xs" onClick={() => setEdit(null)}>
                  Отмена
                </Button>
                <Button
                  className="px-2 py-1 text-xs"
                  disabled={act.busy || edit.name.trim().length < 2 || !numOk(edit.km) || !numOk(edit.days)}
                  onClick={() =>
                    send(
                      c.id,
                      {
                        ...(edit.name.trim() !== c.name ? { name: edit.name.trim() } : {}),
                        interval_km: edit.km.trim() ? Number(edit.km.trim()) : null,
                        interval_days: edit.days.trim() ? Number(edit.days.trim()) : null,
                      },
                      `Строка «${edit.name.trim()}» сохранена`,
                    )
                  }
                >
                  Сохранить
                </Button>
              </li>
            ) : (
              <li key={c.id} className="flex flex-wrap items-baseline justify-between gap-2 py-1.5">
                <span className={c.hidden ? 'text-slate-400' : ''}>
                  <span className="font-medium">{c.name}</span>
                  <span className="ml-2 text-xs text-slate-500">
                    {c.interval_km !== null ? `каждые ${c.interval_km.toLocaleString('ru-RU')} км` : 'без интервала, км'}
                    {c.interval_days !== null ? `, напоминать через ${c.interval_days} дн.` : ''}
                  </span>
                  {c.hidden && <span className="ml-2 text-xs">(скрыта — нет в списке выбора)</span>}
                </span>
                <span className="flex gap-1">
                  <Button variant="ghost" className="px-2 py-0.5 text-xs" disabled={act.busy} onClick={() => setEdit({ id: c.id, ...draftOf(c) })}>
                    Изменить
                  </Button>
                  <Button
                    variant="ghost"
                    className="px-2 py-0.5 text-xs"
                    disabled={act.busy}
                    onClick={() => send(c.id, { hidden: !c.hidden }, c.hidden ? `Строка «${c.name}» снова в списке` : `Строка «${c.name}» скрыта`)}
                  >
                    {c.hidden ? 'Показать' : 'Скрыть'}
                  </Button>
                </span>
              </li>
            ),
          )}
        </ul>
      )}
      <p className="mt-2 text-sm text-slate-500">Скрытая строка пропадает из списка «Что меняли»; старые записи сохраняют её название.</p>
    </Card>
  )
}
