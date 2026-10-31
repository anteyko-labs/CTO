// «Настройки → Масляная книжка и печать» (задача 003 Н5, ADR-144): что касса печатает сама после
// чека «В сервис», что идёт в чек книжки и в товарный чек, как пишется масло канистрой и разливом,
// идут ли фильтры на срок масла. У каждого филиала своя; правит владелец, изменение — в журнал. Вид
// книжки один — нынешний (задача 002 П12): здесь меняется только содержимое.
import { Link } from 'react-router-dom'
import { Card, CardTitle, ErrorBox, Loading, Notice, PageHeader, toast } from '../components/ui'
import { get, newOpId, put } from '../lib/api'
import { useAction, useLoad } from '../lib/hooks'
import { oilBookSettingsOf, type OilBookSettings, type QtyStyle } from '../lib/oilBookSettings'

const SWITCHES: { key: Exclude<keyof OilBookSettings, 'qty_style'>; title: string; on: string; off: string; group: 'print' | 'book' }[] = [
  { key: 'auto_book', group: 'print', title: 'Чек книжки после чека «В сервис» — печатать сам', on: 'Чек книжки печатается сам', off: 'Чек книжки — только кнопкой «Печать книжки»' },
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
          <Notice tone="sky">
            Следующую замену касса заполняет сама: пробег сейчас + интервал. Если кассир её не прислал, сервер ставит ту же — в чеке, книжке и
            боте одно число.
          </Notice>
        </div>
      )}
    </div>
  )
}
