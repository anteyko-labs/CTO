// Справочник поставщиков; карточка поставщика — SupplierCard (SPEC-02, SPEC-10).
import { useState, type FormEvent } from 'react'
import { useNavigate } from 'react-router-dom'
import { SupplierEditModal } from '../components/SupplierFormModal'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, Missing, PageHeader, Table } from '../components/ui'
import { get, post } from '../lib/api'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import type { Supplier } from '../lib/types'

export default function Suppliers() {
  const list = useLoad(() => get<Supplier[]>('/suppliers'), [])
  const [form, setForm] = useState({ name: '', phone: '', comment: '' })
  const [editing, setEditing] = useState<Supplier | null>(null)
  const { busy, error, run } = useAction()

  const create = (e: FormEvent) => {
    e.preventDefault()
    void run(async () => {
      await post<Supplier>('/suppliers', form)
      setForm({ name: '', phone: '', comment: '' })
      list.reload()
    })
  }

  const navigate = useNavigate()
  const notFilled = missingWithFocus([Boolean(form.name.trim()), 'название', '#supplier-name'])

  return (
    <div>
      <PageHeader title="Поставщики" />

      <Card className="mb-4">
        <form onSubmit={create} className="grid gap-3 sm:grid-cols-[1.5fr_1fr_2fr_auto] sm:items-end">
          <Field label="Название" required>
            <input id="supplier-name" value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} />
          </Field>
          <Field label="Телефон">
            <input type="tel" value={form.phone} onChange={(e) => setForm({ ...form, phone: e.target.value })} />
          </Field>
          <Field label="Комментарий">
            <input value={form.comment} onChange={(e) => setForm({ ...form, comment: e.target.value })} />
          </Field>
          <Button type="submit" disabled={busy || notFilled.length > 0}>
            Добавить
          </Button>
        </form>
        <div className="mt-2 flex flex-col gap-2">
          <Missing items={notFilled} />
          <ErrorBox error={error} />
        </div>
      </Card>

      <Card>
        <ErrorBox error={list.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : !list.data?.length ? (
          <Empty>Поставщиков пока нет. Их выбирают в приходной накладной.</Empty>
        ) : (
          <Table head={['Название', 'Телефон', 'Комментарий', 'Статус', '']}>
            {list.data.map((s) => (
              <tr
                key={s.id}
                className={`cursor-pointer hover:bg-slate-50 ${s.active ? '' : 'text-slate-400'}`}
                onClick={() => navigate(`/suppliers/${s.id}`)}
              >
                <td className="px-2 py-2 font-medium">{s.name}</td>
                <td className="px-2 py-2 whitespace-nowrap">{s.phone || '—'}</td>
                <td className="px-2 py-2">{s.comment || '—'}</td>
                <td className="px-2 py-2">{s.active ? <Badge tone="green">активен</Badge> : <Badge>отключён</Badge>}</td>
                <td className="px-2 py-2 text-right">
                  <Button
                    variant="secondary"
                    className="px-2 py-1 text-xs"
                    onClick={(e) => {
                      e.stopPropagation()
                      setEditing(s)
                    }}
                  >
                    Изменить
                  </Button>
                </td>
              </tr>
            ))}
          </Table>
        )}
      </Card>

      {editing && (
        <SupplierEditModal
          supplier={editing}
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
