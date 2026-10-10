// Справочник поставщиков; карточка поставщика — SupplierCard (SPEC-02, SPEC-10).
import { useState, type FormEvent } from 'react'
import { useNavigate } from 'react-router-dom'
import { SupplierEditModal } from '../components/SupplierFormModal'
import { Icon } from '../components/icons'
import { StockBackLink } from '../components/StockBackLink'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, Missing, Modal, PageHeader, RowMenu, Table, toast } from '../components/ui'
import { get, post } from '../lib/api'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import type { Supplier } from '../lib/types'

export default function Suppliers() {
  const list = useLoad(() => get<Supplier[]>('/suppliers'), [])
  const [form, setForm] = useState({ name: '', phone: '', comment: '' })
  const [editing, setEditing] = useState<Supplier | null>(null)
  const [adding, setAdding] = useState(false)
  const { busy, error, setError, run } = useAction()

  const create = (e: FormEvent) => {
    e.preventDefault()
    void run(async () => {
      await post<Supplier>('/suppliers', form)
      setForm({ name: '', phone: '', comment: '' })
      setAdding(false)
      list.reload()
      toast('Поставщик добавлен')
    })
  }

  const openAdd = () => {
    setError(null)
    setAdding(true)
  }

  const navigate = useNavigate()
  const notFilled = missingWithFocus([Boolean(form.name.trim()), 'название', '#supplier-name'])

  return (
    <div>
      <PageHeader
        title="Поставщики"
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
          <Empty icon="incoming" action={<Button onClick={openAdd}>Добавить поставщика</Button>}>
            Поставщиков пока нет. Их выбирают в приходной накладной.
          </Empty>
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
                <td className="px-2 py-2 text-right" onClick={(e) => e.stopPropagation()}>
                  <RowMenu
                    items={[
                      { label: 'Открыть карточку', onClick: () => navigate(`/suppliers/${s.id}`) },
                      { label: 'Изменить', onClick: () => setEditing(s) },
                    ]}
                  />
                </td>
              </tr>
            ))}
          </Table>
        )}
      </Card>

      {adding && (
        <Modal title="Новый поставщик" onClose={() => setAdding(false)}>
          <form onSubmit={create} className="flex flex-col gap-3">
            <Field label="Название" required>
              <input id="supplier-name" autoFocus value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} />
            </Field>
            <Field label="Телефон">
              <input type="tel" value={form.phone} onChange={(e) => setForm({ ...form, phone: e.target.value })} />
            </Field>
            <Field label="Комментарий">
              <input value={form.comment} onChange={(e) => setForm({ ...form, comment: e.target.value })} />
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
