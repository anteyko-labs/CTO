// Справочник категорий товаров и их характеристик для фильтров (SPEC-02); строка масляной книжки
// и её интервал у категории (SPEC-27).
import { useState, type FormEvent } from 'react'
import { Icon } from '../components/icons'
import { StockBackLink } from '../components/StockBackLink'
import { ExportButton } from '../components/ExportButton'
import { listSheet, xl, type ExportCol } from '../lib/exportTable'
import { CrossPickCategory } from '../components/CrossPickCategory'
import { ProductFormModal } from '../components/ProductFormModal'
import { PointsRuleField } from '../components/PointsRuleField'
import { parsePointsRule, pointsChanged, pointsDraft } from '../lib/points'
import { Badge, Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, Missing, Modal, PageHeader, Table, toast } from '../components/ui'
import { get, patch, post } from '../lib/api'
import { useUser } from '../lib/auth'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import { useCan } from '../lib/permissions'
import { categorySlotText } from '../lib/oilBook'
import { BOOK_SLOT_LABELS, KIND_LABELS, type AttributeDef, type BookSlot, type Category, type CategoryKind } from '../lib/types'

const KINDS = Object.keys(KIND_LABELS) as CategoryKind[]

const SLOTS = Object.keys(BOOK_SLOT_LABELS) as BookSlot[]


const TYPE_LABELS: Record<AttributeDef['type'], string> = {
  text: 'Текст',
  number: 'Число',
  select: 'Список',
}

interface AttrRow {
  key: string
  label: string
  type: AttributeDef['type']
  options: string
  filterable: boolean
  /** Выключатель (задача 002 П10): выключенная не видна в карточке и фильтрах, значения остаются. */
  enabled: boolean
}

const toRow = (a: AttributeDef): AttrRow => ({
  key: a.key,
  label: a.label,
  type: a.type,
  options: (a.options ?? []).join(', '),
  filterable: a.filterable,
  enabled: a.enabled !== false,
})

function toDef(r: AttrRow): AttributeDef {
  const def: AttributeDef = { key: r.key.trim(), label: r.label.trim(), type: r.type, filterable: r.filterable, enabled: r.enabled }
  if (!def.key) throw new Error('Укажите ключ каждой характеристики')
  if (!def.label) throw new Error(`Укажите название характеристики «${def.key}»`)
  if (r.type === 'select') {
    const options = r.options
      .split(',')
      .map((o) => o.trim())
      .filter(Boolean)
    if (options.length === 0) throw new Error(`У списка «${def.label}» нет вариантов`)
    def.options = options
  }
  return def
}

function EditModal({ category, onClose, onSaved }: { category: Category; onClose: () => void; onSaved: () => void }) {
  const [name, setName] = useState(category.name)
  const [rows, setRows] = useState<AttrRow[]>(() => category.attributes.map(toRow))
  // Строку книжки и интервал задаёт владелец (SPEC-27).
  const owner = useUser().role === 'owner'
  const [book, setBook] = useState({
    slot: category.book_slot,
    km: category.book_interval_km !== null ? String(category.book_interval_km) : '',
    days: category.book_interval_days !== null ? String(category.book_interval_days) : '',
  })
  const bookChanged =
    book.slot !== category.book_slot ||
    book.km !== (category.book_interval_km !== null ? String(category.book_interval_km) : '') ||
    book.days !== (category.book_interval_days !== null ? String(category.book_interval_days) : '')
  const bookValid = (book.km === '' && book.days === '') || (/^\d+$/.test(book.km) && /^\d+$/.test(book.days))
  // Баллы товаров категории (задача 002, П1) — владелец.
  const [points, setPoints] = useState(() => pointsDraft(category.points_mode, category.points_value))
  const { busy, error, run } = useAction()

  const setRow = (i: number, patchRow: Partial<AttrRow>) =>
    setRows((rs) => rs.map((r, j) => (j === i ? { ...r, ...patchRow } : r)))

  const save = () =>
    run(async () => {
      const rule = parsePointsRule(points.mode, points.text)
      if (typeof rule === 'string') throw new Error(rule)
      await patch<Category>(`/categories/${category.id}`, {
        name,
        attributes: rows.map(toDef),
        ...(owner && pointsChanged(points, category.points_mode, category.points_value) ? { points: rule } : {}),
        ...(owner && bookChanged
          ? {
              book: {
                slot: book.slot,
                interval_km: book.km ? Number(book.km) : null,
                interval_days: book.days ? Number(book.days) : null,
              },
            }
          : {}),
      })
      onSaved()
    })

  return (
    <Modal title="Категория" onClose={onClose} wide>
      <div className="flex flex-col gap-4">
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Название">
            <input value={name} onChange={(e) => setName(e.target.value)} />
          </Field>
          <Field label="Вид" hint="Вид категории не меняется">
            <input value={KIND_LABELS[category.kind]} disabled />
          </Field>
        </div>

        <div className="grid gap-3 rounded-md border border-slate-200 p-3 sm:grid-cols-3">
          <Field label="Строка книжки" hint={category.kind === 'filter' ? 'Фильтр с указанным «Типом» пишется в свою строку; здесь — строка для фильтров без типа' : 'Куда товар этой категории пишется в масляной книжке'}>
            <select
              id="book-slot"
              disabled={!owner}
              value={book.slot}
              onChange={(e) => setBook({ ...book, slot: e.target.value as BookSlot | 'none' })}
            >
              <option value="none">— не в книжку</option>
              {SLOTS.map((s) => (
                <option key={s} value={s}>
                  {BOOK_SLOT_LABELS[s]}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Следующая замена через, км" hint="Пусто — по умолчанию">
            <input
              id="book-km"
              inputMode="numeric"
              disabled={!owner || book.slot === 'none'}
              value={book.km}
              onChange={(e) => setBook({ ...book, km: e.target.value.replace(/\D/g, '') })}
            />
          </Field>
          <Field label="или через, дней" hint={owner ? 'У машины можно задать свой' : 'Меняет владелец'}>
            <input
              id="book-days"
              inputMode="numeric"
              disabled={!owner || book.slot === 'none'}
              value={book.days}
              onChange={(e) => setBook({ ...book, days: e.target.value.replace(/\D/g, '') })}
            />
          </Field>
        </div>

        <div className="sm:max-w-md">
          <PointsRuleField
            inheritLabel="по общей ставке"
            draft={points}
            onChange={setPoints}
            disabled={!owner}
            hint={owner ? 'Для товаров категории без своего правила; общая ставка — в «Телеграм и баллы»' : 'Баллы задаёт владелец'}
          />
        </div>

        <div>
          <div className="mb-2 text-sm font-medium text-slate-700">Характеристики</div>
          {rows.length === 0 && <div className="mb-2 text-sm text-slate-500">Характеристик нет</div>}
          <div className="flex flex-col gap-3">
            {rows.map((r, i) => (
              <div key={i} className="grid gap-2 rounded-md border border-slate-200 p-3 sm:grid-cols-[1fr_1.5fr_1fr]">
                <Field label="Ключ" hint="Латиница, цифры, _">
                  <input value={r.key} onChange={(e) => setRow(i, { key: e.target.value })} />
                </Field>
                <Field label="Название">
                  <input value={r.label} onChange={(e) => setRow(i, { label: e.target.value })} />
                </Field>
                <Field label="Тип">
                  <select value={r.type} onChange={(e) => setRow(i, { type: e.target.value as AttributeDef['type'] })}>
                    {(Object.keys(TYPE_LABELS) as AttributeDef['type'][]).map((t) => (
                      <option key={t} value={t}>
                        {TYPE_LABELS[t]}
                      </option>
                    ))}
                  </select>
                </Field>
                {r.key === 'filter_type' && !r.enabled && (
                  <div className="text-xs text-amber-800 sm:col-span-3">«Тип» выключен — строка книжки у всех товаров категории: «{book.slot === 'none' ? '—' : BOOK_SLOT_LABELS[book.slot]}»</div>
                )}
                {r.type === 'select' && (
                  <div className="sm:col-span-3">
                    <Field label="Варианты через запятую">
                      <input value={r.options} onChange={(e) => setRow(i, { options: e.target.value })} />
                    </Field>
                  </div>
                )}
                <div className="flex flex-wrap items-center justify-between gap-2 sm:col-span-3">
                  <span className="flex flex-wrap items-center gap-4">
                    <label className="inline-flex items-center gap-2 text-sm">
                      <input
                        type="checkbox"
                        className="h-4 w-4"
                        data-testid={`attr-enabled-${r.key}`}
                        checked={r.enabled}
                        // «Тип» фильтра решает строку книжки (ADR-072) — его включает и выключает владелец.
                        disabled={r.key === 'filter_type' && !owner}
                        onChange={(e) => setRow(i, { enabled: e.target.checked })}
                      />
                      Показывать в карточке товара
                    </label>
                    <Checkbox label="Фильтр в списке товаров" checked={r.filterable} onChange={(v) => setRow(i, { filterable: v })} />
                  </span>
                  <Button variant="ghost" className="px-2 py-1 text-xs" onClick={() => setRows((rs) => rs.filter((_, j) => j !== i))}>
                    Удалить
                  </Button>
                </div>
              </div>
            ))}
          </div>
          <Button
            variant="secondary"
            className="mt-3"
            onClick={() => setRows((rs) => [...rs, { key: '', label: '', type: 'text', options: '', filterable: true, enabled: true }])}
          >
            Добавить характеристику
          </Button>
        </div>

        <ErrorBox error={error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={busy || !name.trim() || !bookValid || rows.some((r) => !r.key.trim() || !r.label.trim())} onClick={() => void save()}>
            Сохранить
          </Button>
        </div>
      </div>
    </Modal>
  )
}

export default function Categories() {
  // Категории — часть товаров: заводит и правит владелец или администратор с правом (ADR-097).
  const mayEdit = useCan()('products')
  // Со скрытыми: здесь их показывают снова (задача 003 Н10); в выборе товара их нет.
  const list = useLoad(() => get<Category[]>('/categories?all=true'), [])
  const hide = useAction()
  const setHidden = (c: Category, archived: boolean) =>
    void hide.run(async () => {
      await patch<Category>(`/categories/${c.id}`, { archived })
      list.reload()
      toast(archived ? `Категория «${c.name}» скрыта` : `Категория «${c.name}» снова в выборе`)
    })
  const [form, setForm] = useState<{ name: string; kind: CategoryKind }>({ name: '', kind: 'other' })
  const [editing, setEditing] = useState<Category | null>(null)
  const [adding, setAdding] = useState(false)
  // «+ Товар»: форма товара с уже выбранной категорией (SPEC-27).
  const [newProductIn, setNewProductIn] = useState<Category | null>(null)
  // «Подобрать кросс-номера» для всей категории фильтров (задача 002 П10).
  const [crossIn, setCrossIn] = useState<Category | null>(null)
  const { busy, error, setError, run } = useAction()

  const create = (e: FormEvent) => {
    e.preventDefault()
    void run(async () => {
      await post<Category>('/categories', { name: form.name, kind: form.kind })
      setForm({ name: '', kind: form.kind })
      setAdding(false)
      list.reload()
      toast('Категория добавлена')
    })
  }

  const openAdd = () => {
    setError(null)
    setAdding(true)
  }

  const notFilled = missingWithFocus([Boolean(form.name.trim()), 'название', '#category-name'])

  return (
    <div>
      <PageHeader
        title="Категории"
        back={<StockBackLink />}
        actions={
          <>
            <ExportButton file="Категории" disabled={!list.data?.length} sheets={() => [listSheet('Категории', CATEGORY_COLS, list.data ?? [])]} />
            {mayEdit && (
              <Button onClick={openAdd}>
                <Icon name="plus" className="h-4 w-4" />
                Добавить
              </Button>
            )}
          </>
        }
      />

      <Card>
        <ErrorBox error={list.error ?? hide.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : !list.data?.length ? (
          <Empty icon="products" action={mayEdit && <Button onClick={openAdd}>Добавить категорию</Button>}>
            Категорий пока нет. Начните с одной: «Масла» или «Фильтры» — товар заводится внутри категории.
          </Empty>
        ) : (
          <Table head={['Название', 'Вид', 'Строка книжки', 'Характеристик', '']}>
            {list.data.map((c) => (
              <tr
                key={c.id}
                className={`${mayEdit ? 'cursor-pointer hover:bg-slate-50' : ''} ${c.archived ? 'text-slate-400' : ''}`}
                onClick={() => mayEdit && setEditing(c)}
              >
                <td className="px-2 py-2 font-medium">
                  {c.name} {c.archived && <Badge>скрыта</Badge>}
                </td>
                <td className="px-2 py-2">{KIND_LABELS[c.kind]}</td>
                <td className="px-2 py-2">
                  {categorySlotText(c).main}
                  {categorySlotText(c).note && <span className="block text-xs text-slate-500">{categorySlotText(c).note}</span>}
                  {c.book_interval_km !== null && c.book_interval_days !== null && (
                    <span className="ml-1 text-xs text-slate-500">
                      ({c.book_interval_km.toLocaleString('ru-RU')} км / {c.book_interval_days} дн.)
                    </span>
                  )}
                </td>
                <td className="px-2 py-2">{c.attributes.length}</td>
                <td className="px-2 py-2 text-right whitespace-nowrap">
                  {mayEdit && c.kind === 'filter' && (
                    <Button
                      variant="secondary"
                      className="mr-2 px-2 py-1 text-xs"
                      aria-label={`Подобрать кросс-номера в категории ${c.name}`}
                      onClick={(e) => {
                        e.stopPropagation()
                        setCrossIn(c)
                      }}
                    >
                      Кросс-номера
                    </Button>
                  )}
                  {mayEdit && (
                    <Button
                      variant="ghost"
                      className="mr-1 px-2 py-1 text-xs"
                      disabled={hide.busy}
                      title={c.archived ? 'Вернуть категорию в выбор товара' : 'Скрыть из выбора; можно, когда в ней нет товаров в продаже'}
                      aria-label={c.archived ? `Показать категорию ${c.name}` : `Скрыть категорию ${c.name}`}
                      onClick={(e) => {
                        e.stopPropagation()
                        setHidden(c, !c.archived)
                      }}
                    >
                      {c.archived ? 'Показать' : 'Скрыть'}
                    </Button>
                  )}
                  {mayEdit && !c.archived && (
                    <Button
                      variant="secondary"
                      className="px-2 py-1 text-xs"
                      aria-label={`Новый товар в категории ${c.name}`}
                      onClick={(e) => {
                        e.stopPropagation()
                        setNewProductIn(c)
                      }}
                    >
                      + Товар
                    </Button>
                  )}
                </td>
              </tr>
            ))}
          </Table>
        )}
      </Card>

      {adding && (
        <Modal title="Новая категория" onClose={() => setAdding(false)}>
          <form onSubmit={create} className="flex flex-col gap-3">
            <Field label="Название" required>
              <input id="category-name" autoFocus value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} />
            </Field>
            <Field label="Вид" hint="Масло по умолчанию в литрах, остальное — в штуках (у товара можно выбрать другое). Стандартные характеристики добавляются автоматически.">
              <select value={form.kind} onChange={(e) => setForm({ ...form, kind: e.target.value as CategoryKind })}>
                {KINDS.map((k) => (
                  <option key={k} value={k}>
                    {KIND_LABELS[k]}
                  </option>
                ))}
              </select>
            </Field>
            <Missing items={notFilled} />
            <ErrorBox error={error} />
            <div className="flex justify-end gap-2">
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

      {newProductIn && (
        <ProductFormModal
          presetCategoryId={newProductIn.id}
          onClose={() => setNewProductIn(null)}
          onSaved={(p) => {
            setNewProductIn(null)
            toast(`Товар «${p.name}» заведён в категории «${newProductIn.name}»`)
          }}
        />
      )}

      {crossIn && <CrossPickCategory category={crossIn} onClose={() => setCrossIn(null)} />}

      {editing && (
        <EditModal
          category={editing}
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

/** Категории: строка книжки и интервал — как в таблице; характеристики — списком. */
const CATEGORY_COLS: ExportCol<Category>[] = [
  { label: 'Название', value: (c) => c.name },
  { label: 'Вид', value: (c) => KIND_LABELS[c.kind] },
  { label: 'Строка книжки', value: (c) => [categorySlotText(c).main, categorySlotText(c).note].filter(Boolean).join(', ') },
  { label: 'Интервал, км', value: (c) => (c.book_interval_km !== null ? xl.int(c.book_interval_km) : null) },
  { label: 'Интервал, дней', value: (c) => (c.book_interval_days !== null ? xl.int(c.book_interval_days) : null) },
  { label: 'Характеристик', value: (c) => xl.int(c.attributes.length) },
  { label: 'Характеристики', value: (c) => c.attributes.map((a) => a.label).join(', ') },
  { label: 'Статус', value: (c) => (c.archived ? 'скрыта' : '') },
]
