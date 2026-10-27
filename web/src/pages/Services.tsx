// Справочник услуг (SPEC-02, SPEC-07, SPEC-30 §2): цена клиенту, «бесплатно с нашим маслом» и
// ставка мастеру за одну услугу — вид замены в чеке «В сервис» (ADR-096). На новой точке владелец
// загружает услуги из выгрузки товаров старой программы (Excel) — отмеченные строки разом.
import { useState, type FormEvent } from 'react'
import { DirHint } from '../components/DirHint'
import { Icon } from '../components/icons'
import { Badge, Button, Card, CardTitle, Checkbox, Empty, ErrorBox, Field, Loading, Missing, Modal, Money, PageHeader, RowMenu, Table, toast } from '../components/ui'
import { get, newOpId, patch, post } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatSom, parseSom, plural, somInput } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import { BOOK_SLOT_LABELS, type BookSlot, type Category, type Service } from '../lib/types'
import { mapUmagRows, preselected, serviceCandidates, type ServiceCandidate } from '../lib/umag'
import { readXlsx } from '../lib/xlsx'

const FEE_HINT = 'Ставка мастеру за одну такую работу: в чеке — количество × ставка. Ставку задаёт владелец'
const FREE_HINT =
  'В чеке нашего масла этой категории не меньше канистры — услуга бесплатна; клиент со своим маслом или с доливом платит цену клиенту'
const ABOUT =
  'Вид замены выбирается в каждом чеке «В сервис» — без него чек не провести. Мастеру начисляется ставка каждой услуги чека. ' +
  'У услуги «Бесплатно с нашим маслом» цена — 0, если в чеке нашего масла её категории не меньше канистры; иначе — цена клиенту. ' +
  'Если клиент вернёт масло и его останется меньше канистры, цена замены удержится из возврата.'

const TAKEAWAY_LABEL = 'Доливка — продаётся на вынос'
const TAKEAWAY_HINT = 'Касса пробивает её «На вынос» без клиента, машины и книжки; мастер — по желанию. В окне «Вид замены» её нет'

/** Флажок «Доливка — на вынос» (ADR-118): меняет владелец; у новой услуги — по названию («долив»). */
function TakeawayBox({ checked, onChange }: { checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <div className="flex flex-col gap-1" data-testid="service-takeaway">
      <Checkbox label={TAKEAWAY_LABEL} checked={checked} onChange={onChange} />
      <div className="-mt-1 pl-7 text-xs text-slate-500">{TAKEAWAY_HINT}</div>
    </div>
  )
}

/** Пустое поле «Мастеру» — ставка по умолчанию (30 сом за каждую услугу, ADR-112): сервер ставит её сам. */
function amounts(price: string, fee: string): { price_tyiyn: number; master_fee_tyiyn?: number } {
  const p = parseSom(price)
  if (p === null) throw new Error('Неверная цена')
  if (!fee.trim()) return { price_tyiyn: p }
  const f = parseSom(fee)
  if (f === null) throw new Error('Неверное начисление мастеру')
  return { price_tyiyn: p, master_fee_tyiyn: f }
}

const SLOTS = Object.keys(BOOK_SLOT_LABELS) as BookSlot[]
const BOOK_HINT =
  'Что меняет услуга: по этой строке касса спросит «Следующая замена» (пробег сейчас + интервал) и запишет её в книжку. «Прочие работы» — без напоминания'

/** Строка книжки и интервал услуги (SPEC-34): поля формы. */
interface BookForm {
  slot: BookSlot | ''
  km: string
  days: string
}

function bookPayload(b: BookForm, keepEmpty: boolean): Record<string, unknown> {
  const km = b.km.trim() ? Number(b.km) : null
  const days = b.days.trim() ? Number(b.days) : null
  if ((km === null) !== (days === null)) throw new Error('Интервал: укажите и км, и дни — или оставьте оба пустыми')
  return {
    ...(b.slot ? { book_slot: b.slot } : {}),
    ...(km !== null && days !== null ? { book_interval_km: km, book_interval_days: days } : keepEmpty ? { book_interval_reset: true } : {}),
  }
}

/** Строка книжки услуги и её интервал (меняет владелец). */
function BookFields({ value, onChange, byName, idPrefix }: { value: BookForm; onChange: (b: BookForm) => void; byName?: boolean; idPrefix: string }) {
  const slot = value.slot || null
  return (
    <div className="flex flex-col gap-2">
      <Field label="Строка книжки" hint={BOOK_HINT}>
        <select id={`${idPrefix}-slot`} value={value.slot} onChange={(e) => onChange({ ...value, slot: e.target.value as BookSlot | '' })}>
          {byName && <option value="">по названию услуги</option>}
          {SLOTS.map((s) => (
            <option key={s} value={s}>
              {BOOK_SLOT_LABELS[s]}
            </option>
          ))}
        </select>
      </Field>
      {slot !== 'other' && (
        <div className="flex flex-wrap gap-3">
          <Field label="Интервал, км" hint="Пусто — по строке книжки">
            <input id={`${idPrefix}-km`} className="w-32" inputMode="numeric" value={value.km} onChange={(e) => onChange({ ...value, km: e.target.value.replace(/\D/g, '') })} />
          </Field>
          <Field label="или дней">
            <input id={`${idPrefix}-days`} className="w-24" inputMode="numeric" value={value.days} onChange={(e) => onChange({ ...value, days: e.target.value.replace(/\D/g, '') })} />
          </Field>
        </div>
      )}
    </div>
  )
}

/** Категории масла: у флажка «Бесплатно с нашим маслом» выбирается, с каким маслом (ADR-104). */
function useOilCategories() {
  return useLoad(() => get<Category[]>('/categories').then((list) => list.filter((c) => c.kind === 'oil')), [])
}

/** Флажок «Бесплатно с нашим маслом» и категория масла рядом с ним. */
function FreeWithOil({
  free,
  category,
  categories,
  onChange,
  id,
}: {
  free: boolean
  category: string
  categories: Category[]
  onChange: (free: boolean, category: string) => void
  id: string
}) {
  return (
    <div className="flex flex-col gap-2">
      <Checkbox label="Бесплатно с нашим маслом" checked={free} onChange={(v) => onChange(v, category)} />
      {free && (
        <div className="pl-7">
          <Field label="Категория масла" required hint="Например, «Моторные масла» — замена в моторе, «Трансмиссионные масла» — в коробке">
            <select id={id} value={category} onChange={(e) => onChange(free, e.target.value)}>
              <option value="">— выберите —</option>
              {categories.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name}
                </option>
              ))}
            </select>
          </Field>
        </div>
      )}
      <div className="-mt-1 pl-7 text-xs text-slate-500">{FREE_HINT}</div>
    </div>
  )
}

function EditModal({ service, onClose, onSaved }: { service: Service; onClose: () => void; onSaved: () => void }) {
  const owner = useUser().role === 'owner'
  const categories = useOilCategories()
  const [form, setForm] = useState({
    name: service.name,
    price: somInput(service.price_tyiyn),
    fee: somInput(service.master_fee_tyiyn),
    free: Boolean(service.free_with_oil),
    oilCat: service.oil_category_id ?? '',
    active: service.active,
    takeaway: Boolean(service.takeaway),
  })
  const [book, setBook] = useState<BookForm>({
    slot: service.book_slot ?? 'other',
    km: service.book_interval_km != null ? String(service.book_interval_km) : '',
    days: service.book_interval_days != null ? String(service.book_interval_days) : '',
  })
  const { busy, error, run } = useAction()

  const save = () =>
    run(async () => {
      await patch<Service>(`/services/${service.id}`, {
        name: form.name,
        ...amounts(form.price, form.fee),
        free_with_oil: form.free,
        ...(owner && form.oilCat ? { oil_category_id: form.oilCat } : {}),
        ...(owner ? bookPayload(book, true) : {}),
        ...(owner ? { takeaway: form.takeaway } : {}),
        active: form.active,
      })
      onSaved()
    })

  return (
    <Modal title="Услуга" onClose={onClose}>
      <div className="flex flex-col gap-3">
        <Field label="Название">
          <input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} />
        </Field>
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Цена клиенту, с" hint={owner ? 'Со своим маслом' : 'Цену меняет владелец'}>
            <input id="service-edit-price" inputMode="decimal" disabled={!owner} value={form.price} onChange={(e) => setForm({ ...form, price: e.target.value })} />
          </Field>
          <Field label="Мастеру, с" hint={FEE_HINT}>
            <input id="service-edit-fee" inputMode="decimal" disabled={!owner} value={form.fee} onChange={(e) => setForm({ ...form, fee: e.target.value })} />
          </Field>
        </div>
        {owner ? (
          <FreeWithOil
            id="service-edit-oil"
            free={form.free}
            category={form.oilCat}
            categories={categories.data ?? []}
            onChange={(free, oilCat) => setForm({ ...form, free, oilCat })}
          />
        ) : (
          <div className="text-sm text-slate-600">{form.free ? 'Бесплатно с нашим маслом' : 'Платная и с нашим маслом'} — меняет владелец</div>
        )}
        {owner ? (
          <BookFields idPrefix="service-edit-book" value={book} onChange={setBook} />
        ) : (
          <div className="text-sm text-slate-600">Строка книжки: {BOOK_SLOT_LABELS[service.book_slot ?? 'other']} — меняет владелец</div>
        )}
        {owner ? (
          <TakeawayBox checked={form.takeaway} onChange={(v) => setForm({ ...form, takeaway: v })} />
        ) : (
          service.takeaway && <div className="text-sm text-slate-600">{TAKEAWAY_LABEL} — меняет владелец</div>
        )}
        <Checkbox label="Активна" checked={form.active} onChange={(v) => setForm({ ...form, active: v })} />
        <ErrorBox error={error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={busy || !form.name.trim() || (owner && form.free && !form.oilCat)} onClick={() => void save()}>
            Сохранить
          </Button>
        </div>
      </div>
    </Modal>
  )
}

/** Прежняя ставка за чек (ADR-027) новым чекам не начисляется: её получает только чек без сети со
 *  старого устройства «в сервис» без вида замены (ADR-096). Здесь — только пояснение. */
function OldCheckFee() {
  const settings = useLoad(() => get<{ oil_change_master_fee_tyiyn: number }>('/settings/sales'), [])
  const fee = settings.data?.oil_change_master_fee_tyiyn
  if (fee === undefined) return null
  return (
    <div className="mb-4 text-xs text-slate-500" data-testid="old-check-fee">
      Чек без сети со старого устройства «в сервис» без вида замены получит прежнюю ставку за чек — {formatSom(fee)}; владелец увидит его в
      уведомлениях.
    </div>
  )
}

/** Услуги из выгрузки старой программы: строки с отметками, похожие на услуги отмечены заранее. */
function ImportModal({ candidates, onClose, onDone }: { candidates: ServiceCandidate[]; onClose: () => void; onDone: () => void }) {
  const [picked, setPicked] = useState(() => preselected(candidates))
  const [q, setQ] = useState('')
  // Один op_id на открытое окно: повтор после потерянного ответа не заведёт услуги дважды.
  const [opId] = useState(newOpId)
  const { busy, error, run } = useAction()
  const needle = q.trim().toLowerCase()
  const shown = candidates.filter((c) => !needle || c.name.toLowerCase().includes(needle))
  const toggle = (name: string, on: boolean) =>
    setPicked((prev) => {
      const next = new Set(prev)
      if (on) next.add(name)
      else next.delete(name)
      return next
    })
  const items = candidates.filter((c) => picked.has(c.name) && !c.exists)

  const save = () =>
    void run(async () => {
      const r = await post<{ created: number; skipped: string[] }>('/services/import', {
        op_id: opId,
        items: items.map((c) => ({ name: c.name, price_tyiyn: c.price_tyiyn })),
      })
      toast(
        `Заведено ${r.created} ${plural(r.created, ['услуга', 'услуги', 'услуг'])}` +
          (r.skipped.length ? `, пропущено (уже есть): ${r.skipped.length}` : ''),
      )
      onDone()
    })

  return (
    <Modal title="Услуги из Excel" wide onClose={onClose}>
      <div className="flex flex-col gap-3">
        <div className="text-sm text-slate-600">
          Отмечены строки, похожие на услуги: категория «Незаданные» или без закупочной цены, единица — штуки. Уже заведённые услуги не
          загружаются. Ставку мастеру задайте потом в карточке услуги.
        </div>
        <input aria-label="Поиск по файлу" placeholder="Поиск" value={q} onChange={(e) => setQ(e.target.value)} />
        <div className="max-h-[55vh] overflow-y-auto">
          <Table head={['', 'Название', 'Цена', 'Категория', 'Ед.']}>
            {shown.map((c) => (
              <tr key={c.name} className={c.exists ? 'text-slate-400' : ''}>
                <td className="px-2 py-2">
                  <input
                    type="checkbox"
                    className="h-4 w-4"
                    aria-label={`Загрузить «${c.name}»`}
                    disabled={c.exists}
                    checked={!c.exists && picked.has(c.name)}
                    onChange={(e) => toggle(c.name, e.target.checked)}
                  />
                </td>
                <td className="px-2 py-2">
                  {c.name}
                  {c.exists && <span className="ml-2 text-xs">уже есть</span>}
                </td>
                <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">{formatSom(c.price_tyiyn)}</td>
                <td className="px-2 py-2 text-slate-600">{c.category || '—'}</td>
                <td className="px-2 py-2 text-slate-600">{c.unit || '—'}</td>
              </tr>
            ))}
          </Table>
        </div>
        <ErrorBox error={error} />
        <div className="flex flex-wrap items-center justify-end gap-2">
          <span className="mr-auto text-sm text-slate-600">
            Отмечено: <b>{items.length}</b>
          </span>
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={busy || items.length === 0} onClick={save}>
            Загрузить {items.length > 0 ? `${items.length} ${plural(items.length, ['услугу', 'услуги', 'услуг'])}` : ''}
          </Button>
        </div>
      </div>
    </Modal>
  )
}

export default function Services() {
  const owner = useUser().role === 'owner'
  const list = useLoad(() => get<Service[]>('/services'), [])
  // Флажок доливки: пока владелец его не трогал (null) — по названию услуги, как на сервере.
  const [form, setForm] = useState<{ name: string; price: string; fee: string; free: boolean; oilCat: string; takeaway: boolean | null }>({ name: '', price: '', fee: '', free: false, oilCat: '', takeaway: null })
  const takeawayShown = form.takeaway ?? /долив/i.test(form.name)
  // Ставка мастеру по умолчанию — за каждую услугу (ADR-112): подсказка в пустом поле.
  const salesSettings = useLoad(() => get<{ oil_change_master_fee_tyiyn: number }>('/settings/sales'), [])
  const defaultFee = salesSettings.data?.oil_change_master_fee_tyiyn
  const [book, setBook] = useState<BookForm>({ slot: '', km: '', days: '' })
  const categories = useOilCategories()
  const catName = (id: string | null | undefined) => categories.data?.find((c) => c.id === id)?.name
  const [adding, setAdding] = useState(false)
  const [q, setQ] = useState('')
  const [editing, setEditing] = useState<Service | null>(null)
  const { busy, error, run } = useAction()
  const toggle = useAction()
  const upload = useAction()
  const [importing, setImporting] = useState<ServiceCandidate[] | null>(null)

  const onFile = (file: File) =>
    void upload.run(async () => {
      const found = serviceCandidates(mapUmagRows(await readXlsx(file)), (list.data ?? []).map((s) => s.name))
      if (found.length === 0) throw new Error('В файле нет строк с названием')
      setImporting(found)
    })

  const create = (e: FormEvent) => {
    e.preventDefault()
    void run(async () => {
      await post<Service>('/services', {
        name: form.name,
        ...amounts(form.price, form.fee),
        free_with_oil: owner && form.free,
        ...(owner && form.free && form.oilCat ? { oil_category_id: form.oilCat } : {}),
        ...(owner ? bookPayload(book, false) : {}),
        ...(owner && form.takeaway !== null ? { takeaway: form.takeaway } : {}),
      })
      setForm({ name: '', price: '', fee: '', free: false, oilCat: '', takeaway: null })
      setBook({ slot: '', km: '', days: '' })
      setAdding(false)
      list.reload()
    })
  }

  const setActive = (s: Service, active: boolean) =>
    void toggle.run(async () => {
      await patch<Service>(`/services/${s.id}`, { name: s.name, price_tyiyn: s.price_tyiyn, master_fee_tyiyn: s.master_fee_tyiyn, free_with_oil: s.free_with_oil, active })
      toast(active ? 'Услуга включена' : 'Услуга отключена')
      list.reload()
    })

  const notFilled = missingWithFocus(
    [Boolean(form.name.trim()), 'название', '#service-name'],
    [Boolean(form.price.trim()), 'цену', '#service-price'],
    [!form.free || Boolean(form.oilCat), 'категорию масла', '#service-oil'],
  )
  const needle = q.trim().toLowerCase()
  const rows = (list.data ?? []).filter((s) => !needle || s.name.toLowerCase().includes(needle))

  return (
    <div>
      <PageHeader
        title="Услуги"
        actions={
          <div className="flex flex-wrap gap-2">
            {owner && (
              <label className={`inline-flex min-h-[42px] cursor-pointer items-center rounded-md border border-slate-300 bg-white px-4 py-2 text-sm font-medium text-slate-700 hover:bg-slate-50 ${upload.busy ? 'opacity-60' : ''}`}>
                {upload.busy ? 'Читаю файл…' : 'Загрузить из Excel'}
                <input
                  type="file"
                  accept=".xlsx"
                  className="hidden"
                  disabled={upload.busy}
                  onChange={(e) => {
                    const f = e.target.files?.[0]
                    if (f) onFile(f)
                    e.target.value = ''
                  }}
                />
              </label>
            )}
            <Button onClick={() => setAdding(true)}>
              <Icon name="plus" /> Добавить
            </Button>
          </div>
        }
      />

      <OldCheckFee />

      <Card className="[&_th:nth-child(2)]:text-right [&_th:nth-child(4)]:text-right">
        <CardTitle
          actions={
            (list.data?.length ?? 0) > 5 && (
              <input aria-label="Поиск услуги" className="w-full sm:w-64" placeholder="Поиск" value={q} onChange={(e) => setQ(e.target.value)} />
            )
          }
        >
          Услуги в кассе
          <DirHint text={ABOUT} />
        </CardTitle>
        <ErrorBox error={list.error ?? toggle.error ?? upload.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : !list.data?.length ? (
          <Empty
            icon="services"
            action={
              <Button onClick={() => setAdding(true)}>
                <Icon name="plus" /> Добавить услугу
              </Button>
            }
          >
            Услуг пока нет. Добавьте виды замены, например «Замена масла в моторе» и «Замена масла в коробке», — они сразу появятся в кассе.
          </Empty>
        ) : rows.length === 0 ? (
          <Empty>Ничего не найдено</Empty>
        ) : (
          <Table head={['Название', 'Цена клиенту', 'С нашим маслом', 'Мастеру', 'Книжка', 'Статус', '']}>
            {rows.map((s) => (
              <tr
                key={s.id}
                className={`cursor-pointer hover:bg-slate-50 ${s.active ? '' : 'text-slate-400'}`}
                onClick={() => setEditing(s)}
              >
                <td className="px-2 py-2 font-medium">
                  {s.name}
                  {s.takeaway && (
                    <span className="ml-2 align-middle">
                      <Badge tone="amber">на вынос</Badge>
                    </span>
                  )}
                </td>
                <td className="px-2 py-2 md:text-right">
                  <Money value={s.price_tyiyn} />
                </td>
                <td className="px-2 py-2">
                  {!s.free_with_oil ? (
                    <span className="text-slate-500">цена клиенту</span>
                  ) : s.oil_category_id ? (
                    <span className="flex flex-wrap items-center gap-1">
                      <Badge tone="sky">бесплатно</Badge>
                      <span className="text-xs text-slate-600">{catName(s.oil_category_id) ?? ''}</span>
                    </span>
                  ) : (
                    <Badge tone="amber">выберите категорию масла</Badge>
                  )}
                </td>
                <td className="px-2 py-2 md:text-right">
                  <Money value={s.master_fee_tyiyn} zero="—" />
                </td>
                <td className="px-2 py-2 text-sm">
                  {!s.book_slot || s.book_slot === 'other' ? (
                    <span className="text-slate-500">без напоминания</span>
                  ) : (
                    <span>
                      {BOOK_SLOT_LABELS[s.book_slot]}
                      {s.interval_km != null && <span className="block text-xs text-slate-500">через {s.interval_km.toLocaleString('ru-RU')} км</span>}
                    </span>
                  )}
                </td>
                <td className="px-2 py-2">{s.active ? <Badge tone="green">активна</Badge> : <Badge>отключена</Badge>}</td>
                <td className="overflow-visible! px-2 py-1 text-right" onClick={(ev) => ev.stopPropagation()}>
                  <RowMenu
                    items={[
                      { label: 'Изменить', onClick: () => setEditing(s) },
                      s.active
                        ? { label: 'Отключить', danger: true, onClick: () => setActive(s, false) }
                        : { label: 'Включить', onClick: () => setActive(s, true) },
                    ]}
                  />
                </td>
              </tr>
            ))}
          </Table>
        )}
      </Card>

      {adding && (
        <Modal title="Новая услуга" onClose={() => setAdding(false)}>
          <form onSubmit={create} className="flex flex-col gap-3">
            <Field label="Название" required>
              <input id="service-name" autoFocus value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} />
            </Field>
            <div className="flex flex-wrap gap-3">
              <Field label="Цена клиенту, с" required hint="Со своим маслом">
                <input id="service-price" className="w-40" inputMode="decimal" value={form.price} onChange={(e) => setForm({ ...form, price: e.target.value })} />
              </Field>
              <label className="flex flex-col gap-1 text-sm">
                <span className="font-medium text-slate-700">
                  Мастеру, с
                  <DirHint text={FEE_HINT} />
                </span>
                <input
                  id="service-fee"
                  className="w-40"
                  inputMode="decimal"
                  disabled={!owner}
                  placeholder={defaultFee !== undefined ? `по умолчанию ${formatSom(defaultFee)}` : 'по умолчанию'}
                  value={form.fee}
                  onChange={(e) => setForm({ ...form, fee: e.target.value })}
                />
              </label>
            </div>
            {owner && (
              <FreeWithOil
                id="service-oil"
                free={form.free}
                category={form.oilCat}
                categories={categories.data ?? []}
                onChange={(free, oilCat) => setForm({ ...form, free, oilCat })}
              />
            )}
            {owner && <BookFields idPrefix="service-book" value={book} onChange={setBook} byName />}
            {owner && <TakeawayBox checked={takeawayShown} onChange={(v) => setForm({ ...form, takeaway: v })} />}
            <ErrorBox error={error} />
            <div className="flex flex-wrap items-center justify-end gap-2">
              <Missing items={notFilled} className="mr-auto" />
              <Button variant="secondary" onClick={() => setAdding(false)}>
                Отмена
              </Button>
              <Button type="submit" disabled={busy || notFilled.length > 0}>
                Добавить
              </Button>
            </div>
          </form>
        </Modal>
      )}

      {importing && (
        <ImportModal
          candidates={importing}
          onClose={() => setImporting(null)}
          onDone={() => {
            setImporting(null)
            list.reload()
          }}
        />
      )}

      {editing && (
        <EditModal
          service={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null)
            list.reload()
          }}
        />
      )}
    </div>
  )
}
