// Справочник услуг и ставка мастера за замену (SPEC-02, SPEC-07).
import { useState, type FormEvent } from 'react'
import { Badge, Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, Missing, Modal, PageHeader, Table, toast } from '../components/ui'
import { get, patch, post, put } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatSom, parseSom, somInput } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import type { Service } from '../lib/types'

const FEE_HINT = 'Начисляется мастеру за каждую такую работу в чеке'

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

/** Ставка мастера за чек с заменой: замена в чеке строкой не печатается (ADR-027). */
function OilChangeFee() {
  const owner = useUser().role === 'owner'
  const settings = useLoad(() => get<{ oil_change_master_fee_tyiyn: number }>('/settings/sales'), [])
  const [fee, setFee] = useState<string | null>(null)
  const save = useAction()
  const current = settings.data?.oil_change_master_fee_tyiyn ?? 0
  const value = fee ?? somInput(current)

  const submit = () =>
    save.run(async () => {
      const v = parseSom(value)
      if (v === null) throw new Error('Неверная ставка')
      await put('/settings/sales', { oil_change_master_fee_tyiyn: v })
      setFee(null)
      settings.reload()
      toast('Ставка сохранена')
    })

  return (
    <Card className="mb-4">
      <div className="grid gap-3 sm:grid-cols-[1fr_auto] sm:items-end">
        <Field
          label="Мастеру за замену, с"
          hint="Одна ставка на чек с отметкой «в сервис». Замена отдельной строкой в чеке не печатается, цена масла та же."
        >
          <input inputMode="decimal" disabled={!owner || settings.loading} value={value} onChange={(e) => setFee(e.target.value)} />
        </Field>
        {owner && (
          <Button disabled={save.busy || fee === null || !value.trim()} onClick={() => void submit()}>
            Сохранить
          </Button>
        )}
      </div>
      <div className="mt-2">
        <ErrorBox error={settings.error ?? save.error} />
      </div>
    </Card>
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

  const notFilled = missingWithFocus(
    [Boolean(form.name.trim()), 'название', '#service-name'],
    [Boolean(form.price.trim()), 'цену', '#service-price'],
  )

  return (
    <div>
      <PageHeader title="Услуги" />

      <OilChangeFee />

      <Card className="mb-4">
        <form onSubmit={create} className="grid gap-3 sm:grid-cols-[2fr_1fr_1fr_auto] sm:items-end">
          <Field label="Название" required>
            <input id="service-name" value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} />
          </Field>
          <Field label="Цена, с" required>
            <input id="service-price" inputMode="decimal" value={form.price} onChange={(e) => setForm({ ...form, price: e.target.value })} />
          </Field>
          <Field label="Мастеру, с">
            <input inputMode="decimal" value={form.fee} onChange={(e) => setForm({ ...form, fee: e.target.value })} />
          </Field>
          <Button type="submit" disabled={busy || notFilled.length > 0}>
            Добавить
          </Button>
        </form>
        <p className="mt-2 text-xs text-slate-500">
          {FEE_HINT}. Замена масла здесь не нужна: она отмечается в чеке и платится ставкой выше.
        </p>
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
          <Empty>Услуг пока нет. Замена масла здесь не нужна — она отмечается в чеке.</Empty>
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
