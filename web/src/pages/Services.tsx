import { useState, type FormEvent } from 'react'
import { Badge, Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, Modal, PageHeader, Table } from '../components/ui'
import { get, patch, post } from '../lib/api'
import { formatSom, parseSom, somInput } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import type { Service } from '../lib/types'

const FEE_HINT = 'Начисляется мастеру за каждую такую услугу в продаже'

function amounts(price: string, fee: string): { price_tyiyn: number; master_fee_tyiyn: number } {
  const p = parseSom(price)
  const f = parseSom(fee || '0')
  if (p === null) throw new Error('Неверная цена')
  if (f === null) throw new Error('Неверное начисление мастеру')
  return { price_tyiyn: p, master_fee_tyiyn: f }
}

function EditModal({ service, onClose, onSaved }: { service: Service; onClose: () => void; onSaved: () => void }) {
  const [form, setForm] = useState({
    name: service.name,
    price: somInput(service.price_tyiyn),
    fee: somInput(service.master_fee_tyiyn),
    active: service.active,
  })
  const { busy, error, run } = useAction()

  const save = () =>
    run(async () => {
      await patch<Service>(`/services/${service.id}`, {
        name: form.name,
        ...amounts(form.price, form.fee),
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
          <Field label="Цена, с">
            <input inputMode="decimal" value={form.price} onChange={(e) => setForm({ ...form, price: e.target.value })} />
          </Field>
          <Field label="Мастеру, с" hint={FEE_HINT}>
            <input inputMode="decimal" value={form.fee} onChange={(e) => setForm({ ...form, fee: e.target.value })} />
          </Field>
        </div>
        <Checkbox label="Активна" checked={form.active} onChange={(v) => setForm({ ...form, active: v })} />
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

export default function Services() {
  const list = useLoad(() => get<Service[]>('/services'), [])
  const [form, setForm] = useState({ name: '', price: '', fee: '30' })
  const [editing, setEditing] = useState<Service | null>(null)
  const { busy, error, run } = useAction()

  const create = (e: FormEvent) => {
    e.preventDefault()
    void run(async () => {
      await post<Service>('/services', { name: form.name, ...amounts(form.price, form.fee) })
      setForm({ name: '', price: '', fee: '30' })
      list.reload()
    })
  }

  return (
    <div>
      <PageHeader title="Услуги" />

      <Card className="mb-4">
        <form onSubmit={create} className="grid gap-3 sm:grid-cols-[2fr_1fr_1fr_auto] sm:items-end">
          <Field label="Название">
            <input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} />
          </Field>
          <Field label="Цена, с">
            <input inputMode="decimal" value={form.price} onChange={(e) => setForm({ ...form, price: e.target.value })} />
          </Field>
          <Field label="Мастеру, с">
            <input inputMode="decimal" value={form.fee} onChange={(e) => setForm({ ...form, fee: e.target.value })} />
          </Field>
          <Button type="submit" disabled={busy || !form.name.trim() || !form.price.trim()}>
            Добавить
          </Button>
        </form>
        <p className="mt-2 text-xs text-slate-500">{FEE_HINT}.</p>
        <div className="mt-2">
          <ErrorBox error={error} />
        </div>
      </Card>

      <Card>
        <ErrorBox error={list.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : !list.data?.length ? (
          <Empty>Услуг пока нет</Empty>
        ) : (
          <Table head={['Название', 'Цена', 'Мастеру', 'Статус']}>
            {list.data.map((s) => (
              <tr
                key={s.id}
                className={`cursor-pointer hover:bg-slate-50 ${s.active ? '' : 'text-slate-400'}`}
                onClick={() => setEditing(s)}
              >
                <td className="px-2 py-2 font-medium">{s.name}</td>
                <td className="px-2 py-2 whitespace-nowrap">{formatSom(s.price_tyiyn)}</td>
                <td className="px-2 py-2 whitespace-nowrap">{formatSom(s.master_fee_tyiyn)}</td>
                <td className="px-2 py-2">{s.active ? <Badge tone="green">активна</Badge> : <Badge>отключена</Badge>}</td>
              </tr>
            ))}
          </Table>
        )}
      </Card>

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
