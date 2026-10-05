import { useState, type FormEvent } from 'react'
import { Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, Modal, PageHeader, Table } from '../components/ui'
import { get, patch, post } from '../lib/api'
import { useAction, useLoad } from '../lib/hooks'
import { KIND_LABELS, type AttributeDef, type Category, type CategoryKind } from '../lib/types'

const KINDS = Object.keys(KIND_LABELS) as CategoryKind[]

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
  const { busy, error, run } = useAction()

  const setRow = (i: number, patchRow: Partial<AttrRow>) =>
    setRows((rs) => rs.map((r, j) => (j === i ? { ...r, ...patchRow } : r)))

  const save = () =>
    run(async () => {
      await patch<Category>(`/categories/${category.id}`, { name, attributes: rows.map(toDef) })
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
          <Button disabled={busy} onClick={() => void save()}>
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
  const { busy, error, run } = useAction()

  const create = (e: FormEvent) => {
    e.preventDefault()
    void run(async () => {
      await post<Category>('/categories', { name: form.name, kind: form.kind })
      setForm({ name: '', kind: form.kind })
      list.reload()
    })
  }

  return (
    <div>
      <PageHeader title="Категории" />

      <Card className="mb-4">
        <form onSubmit={create} className="grid gap-3 sm:grid-cols-[2fr_1fr_auto] sm:items-end">
          <Field label="Название">
            <input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} />
          </Field>
          <Field label="Вид">
            <select value={form.kind} onChange={(e) => setForm({ ...form, kind: e.target.value as CategoryKind })}>
              {KINDS.map((k) => (
                <option key={k} value={k}>
                  {KIND_LABELS[k]}
                </option>
              ))}
            </select>
          </Field>
          <Button type="submit" disabled={busy || !form.name.trim()}>
            Добавить
          </Button>
        </form>
        <p className="mt-2 text-xs text-slate-500">
          Масло учитывается в литрах, остальное — в штуках. Стандартные характеристики добавляются автоматически.
        </p>
        <div className="mt-2">
          <ErrorBox error={error} />
        </div>
      </Card>

      <Card>
        <ErrorBox error={list.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : !list.data?.length ? (
          <Empty>Категорий пока нет</Empty>
        ) : (
          <Table head={['Название', 'Вид', 'Характеристик']}>
            {list.data.map((c) => (
              <tr key={c.id} className="cursor-pointer hover:bg-slate-50" onClick={() => setEditing(c)}>
                <td className="px-2 py-2 font-medium">{c.name}</td>
                <td className="px-2 py-2">{KIND_LABELS[c.kind]}</td>
                <td className="px-2 py-2">{c.attributes.length}</td>
              </tr>
            ))}
          </Table>
        )}
      </Card>

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
