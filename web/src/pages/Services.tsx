// Справочник услуг и ставка мастера за замену (SPEC-02, SPEC-07).
import { useState, type FormEvent } from 'react'
import { DirHint } from '../components/DirHint'
import { Icon } from '../components/icons'
import { Badge, Button, Card, CardTitle, Checkbox, Empty, ErrorBox, Field, Loading, Missing, Modal, Money, PageHeader, RowMenu, Table, toast } from '../components/ui'
import { get, patch, post, put } from '../lib/api'
import { useUser } from '../lib/auth'
import { parseSom, somInput } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import type { Service } from '../lib/types'

const FEE_HINT = 'Из цены услуги — мастеру, остальное остаётся в кассе. Ставку задаёт владелец'
const OIL_FEE_HINT = 'Одна ставка на чек с отметкой «в сервис». Замена отдельной строкой в чеке не печатается, цена масла та же.'
const ABOUT =
  'Услуги появляются в кассе под кнопкой «Услуги». Замена с нашим маслом отмечается в чеке «В сервис» и платится ставкой выше; ' +
  'если клиент привёз своё масло — заведите для этого услугу, например «Замена масла (масло клиента)».'

function amounts(price: string, fee: string): { price_tyiyn: number; master_fee_tyiyn: number } {
  const p = parseSom(price)
  const f = parseSom(fee || '0')
  if (p === null) throw new Error('Неверная цена')
  if (f === null) throw new Error('Неверное начисление мастеру')
  return { price_tyiyn: p, master_fee_tyiyn: f }
}

function EditModal({ service, onClose, onSaved }: { service: Service; onClose: () => void; onSaved: () => void }) {
  const owner = useUser().role === 'owner'
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
          <Field label="Цена, с" hint={owner ? undefined : 'Цену меняет владелец'}>
            <input inputMode="decimal" disabled={!owner} value={form.price} onChange={(e) => setForm({ ...form, price: e.target.value })} />
          </Field>
          <Field label="Мастеру, с" hint={FEE_HINT}>
            <input inputMode="decimal" disabled={!owner} value={form.fee} onChange={(e) => setForm({ ...form, fee: e.target.value })} />
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
      <div className="flex flex-wrap items-end gap-3">
        <label className="flex flex-col gap-1 text-sm">
          <span className="font-medium text-slate-700">
            Мастеру за замену, с
            <DirHint text={OIL_FEE_HINT} />
          </span>
          <input className="w-40" inputMode="decimal" disabled={!owner || settings.loading} value={value} onChange={(e) => setFee(e.target.value)} />
        </label>
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
  const owner = useUser().role === 'owner'
  const list = useLoad(() => get<Service[]>('/services'), [])
  const [form, setForm] = useState({ name: '', price: '', fee: '' })
  const [adding, setAdding] = useState(false)
  const [q, setQ] = useState('')
  const [editing, setEditing] = useState<Service | null>(null)
  const { busy, error, run } = useAction()
  const toggle = useAction()

  const create = (e: FormEvent) => {
    e.preventDefault()
    void run(async () => {
      await post<Service>('/services', { name: form.name, ...amounts(form.price, form.fee) })
      setForm({ name: '', price: '', fee: '' })
      setAdding(false)
      list.reload()
    })
  }

  const setActive = (s: Service, active: boolean) =>
    void toggle.run(async () => {
      await patch<Service>(`/services/${s.id}`, { name: s.name, price_tyiyn: s.price_tyiyn, master_fee_tyiyn: s.master_fee_tyiyn, active })
      toast(active ? 'Услуга включена' : 'Услуга отключена')
      list.reload()
    })

  const notFilled = missingWithFocus(
    [Boolean(form.name.trim()), 'название', '#service-name'],
    [Boolean(form.price.trim()), 'цену', '#service-price'],
  )
  const needle = q.trim().toLowerCase()
  const rows = (list.data ?? []).filter((s) => !needle || s.name.toLowerCase().includes(needle))

  return (
    <div>
      <PageHeader
        title="Услуги"
        actions={
          <Button onClick={() => setAdding(true)}>
            <Icon name="plus" /> Добавить
          </Button>
        }
      />

      <OilChangeFee />

      <Card className="[&_th:nth-child(2)]:text-right [&_th:nth-child(3)]:text-right">
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
        <ErrorBox error={list.error ?? toggle.error} />
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
            Услуг пока нет. Добавьте, например, «Замена масла (масло клиента)» — она сразу появится в кассе.
          </Empty>
        ) : rows.length === 0 ? (
          <Empty>Ничего не найдено</Empty>
        ) : (
          <Table head={['Название', 'Цена', 'Мастеру', 'Статус', '']}>
            {rows.map((s) => (
              <tr
                key={s.id}
                className={`cursor-pointer hover:bg-slate-50 ${s.active ? '' : 'text-slate-400'}`}
                onClick={() => setEditing(s)}
              >
                <td className="px-2 py-2 font-medium">{s.name}</td>
                <td className="px-2 py-2 md:text-right">
                  <Money value={s.price_tyiyn} />
                </td>
                <td className="px-2 py-2 md:text-right">
                  <Money value={s.master_fee_tyiyn} zero="—" />
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
              <Field label="Цена, с" required>
                <input id="service-price" className="w-40" inputMode="decimal" value={form.price} onChange={(e) => setForm({ ...form, price: e.target.value })} />
              </Field>
              <label className="flex flex-col gap-1 text-sm">
                <span className="font-medium text-slate-700">
                  Мастеру, с
                  <DirHint text={FEE_HINT} />
                </span>
                <input className="w-40" inputMode="decimal" disabled={!owner} value={form.fee} onChange={(e) => setForm({ ...form, fee: e.target.value })} />
              </label>
            </div>
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
