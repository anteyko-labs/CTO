// Справочник категорий товаров и их характеристик для фильтров (SPEC-02); строка масляной книжки
// и её интервал у категории (SPEC-27).
import { useState, type FormEvent } from 'react'
import { Icon } from '../components/icons'
import { StockBackLink } from '../components/StockBackLink'
import { ProductFormModal } from '../components/ProductFormModal'
import { Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, Missing, Modal, PageHeader, Table, toast } from '../components/ui'
import { get, patch, post } from '../lib/api'
import { useUser } from '../lib/auth'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
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
}

const toRow = (a: AttributeDef): AttrRow => ({
  key: a.key,
  label: a.label,
  type: a.type,
  options: (a.options ?? []).join(', '),
  filterable: a.filterable,
})

function toDef(r: AttrRow): AttributeDef {
  const def: AttributeDef = { key: r.key.trim(), label: r.label.trim(), type: r.type, filterable: r.filterable }
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
  const { busy, error, run } = useAction()

  const setRow = (i: number, patchRow: Partial<AttrRow>) =>
    setRows((rs) => rs.map((r, j) => (j === i ? { ...r, ...patchRow } : r)))

  const save = () =>
    run(async () => {
      await patch<Category>(`/categories/${category.id}`, {
        name,
        attributes: rows.map(toDef),
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
                {r.type === 'select' && (
                  <div className="sm:col-span-3">
                    <Field label="Варианты через запятую">
                      <input value={r.options} onChange={(e) => setRow(i, { options: e.target.value })} />
                    </Field>
                  </div>
                )}
                <div className="flex flex-wrap items-center justify-between gap-2 sm:col-span-3">
                  <Checkbox label="Фильтр в списке товаров" checked={r.filterable} onChange={(v) => setRow(i, { filterable: v })} />
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
            onClick={() => setRows((rs) => [...rs, { key: '', label: '', type: 'text', options: '', filterable: true }])}
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
  const list = useLoad(() => get<Category[]>('/categories'), [])
  const [form, setForm] = useState<{ name: string; kind: CategoryKind }>({ name: '', kind: 'other' })
  const [editing, setEditing] = useState<Category | null>(null)
  const [adding, setAdding] = useState(false)
  // «+ Товар»: форма товара с уже выбранной категорией (SPEC-27).
  const [newProductIn, setNewProductIn] = useState<Category | null>(null)
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
          <Button onClick={openAdd}>
            <Icon name="plus" className="h-4 w-4" />
            Добавить
          </Button>
        }
      />

      <Card>
        <ErrorBox error={list.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : !list.data?.length ? (
          <Empty icon="products" action={<Button onClick={openAdd}>Добавить категорию</Button>}>
            Категорий пока нет. Начните с одной: «Масла» или «Фильтры» — товар заводится внутри категории.
          </Empty>
        ) : (
          <Table head={['Название', 'Вид', 'Строка книжки', 'Характеристик', '']}>
            {list.data.map((c) => (
              <tr key={c.id} className="cursor-pointer hover:bg-slate-50" onClick={() => setEditing(c)}>
                <td className="px-2 py-2 font-medium">{c.name}</td>
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
                <td className="px-2 py-2 text-right">
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
            <Field label="Вид" hint="Масло учитывается в литрах, остальное — в штуках. Стандартные характеристики добавляются автоматически.">
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
