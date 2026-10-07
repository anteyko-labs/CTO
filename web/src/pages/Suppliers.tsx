import { useState, type FormEvent } from 'react'
import { Badge, Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, Missing, Modal, PageHeader, Table } from '../components/ui'
import { get, patch, post } from '../lib/api'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import type { Supplier } from '../lib/types'

function EditModal({ supplier, onClose, onSaved }: { supplier: Supplier; onClose: () => void; onSaved: () => void }) {
  const [form, setForm] = useState(supplier)
  const { busy, error, run } = useAction()

  const save = () =>
    run(async () => {
      await patch<Supplier>(`/suppliers/${supplier.id}`, {
        name: form.name,
        phone: form.phone,
        comment: form.comment,
        active: form.active,
      })
      onSaved()
    })

  return (
    <Modal title="Поставщик" onClose={onClose}>
      <div className="flex flex-col gap-3">
        <Field label="Название">
          <input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} />
        </Field>
        <Field label="Телефон">
          <input type="tel" value={form.phone} onChange={(e) => setForm({ ...form, phone: e.target.value })} />
        </Field>
        <Field label="Комментарий">
          <textarea rows={3} value={form.comment} onChange={(e) => setForm({ ...form, comment: e.target.value })} />
        </Field>
        <Checkbox label="Активен" checked={form.active} onChange={(v) => setForm({ ...form, active: v })} />
        <ErrorBox error={error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={busy || !form.name.trim()} onClick={() => void save()}>
            Сохранить
          </Button>
        </div>
      </div>
    </Modal>
  )
}

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
          <Table head={['Название', 'Телефон', 'Комментарий', 'Статус']}>
            {list.data.map((s) => (
              <tr
                key={s.id}
                className={`cursor-pointer hover:bg-slate-50 ${s.active ? '' : 'text-slate-400'}`}
                onClick={() => setEditing(s)}
              >
                <td className="px-2 py-2 font-medium">{s.name}</td>
                <td className="px-2 py-2 whitespace-nowrap">{s.phone || '—'}</td>
                <td className="px-2 py-2">{s.comment || '—'}</td>
                <td className="px-2 py-2">{s.active ? <Badge tone="green">активен</Badge> : <Badge>отключён</Badge>}</td>
              </tr>
            ))}
          </Table>
        )}
      </Card>

      {editing && (
        <EditModal
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
