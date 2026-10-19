// «Возврат поставщику» в меню «Склад» (SPEC-29 §25, §9): все возвраты поставщикам за период со
// страницами, сторно и исправлением; «+ Возврат поставщику» — выбор поставщика, потом то же окно,
// что в карточке поставщика. Себестоимость и разницу видит только владелец (сервер не отдаёт).
import { useState } from 'react'
import { Icon } from '../components/icons'
import { SupplierReturnList, SupplierReturnModal } from '../components/SupplierReturns'
import { Button, Card, ErrorBox, Field, Modal, PageHeader } from '../components/ui'
import { get } from '../lib/api'
import { shiftDate, todayBishkek } from '../lib/format'
import { useLoad } from '../lib/hooks'
import type { Supplier } from '../lib/types'

export default function SupplierReturnsPage() {
  const [to, setTo] = useState(todayBishkek)
  const [from, setFrom] = useState(() => shiftDate(todayBishkek(), -30))
  const [listKey, setListKey] = useState(0)
  const [picking, setPicking] = useState(false)
  const [supplierId, setSupplierId] = useState('')
  const [returning, setReturning] = useState<Supplier | null>(null)
  const suppliers = useLoad(() => get<Supplier[]>('/suppliers'), [])
  const active = (suppliers.data ?? []).filter((s) => s.active)

  return (
    <div>
      <PageHeader
        title="Возврат поставщику"
        subtitle="Товар уходит со склада: наш долг поставщику меньше или он отдаёт деньги. Брак — на экране «Брак»."
        actions={
          <Button
            onClick={() => {
              setSupplierId('')
              setPicking(true)
            }}
          >
            <Icon name="plus" className="h-4 w-4" />
            Возврат поставщику
          </Button>
        }
      />
      <Card>
        <div className="mb-4 grid grid-cols-2 gap-3 sm:max-w-md">
          <Field label="С">
            <input type="date" value={from} onChange={(e) => setFrom(e.target.value)} />
          </Field>
          <Field label="По">
            <input type="date" value={to} onChange={(e) => setTo(e.target.value)} />
          </Field>
        </div>
        <SupplierReturnList key={listKey} from={from} to={to} onChanged={() => undefined} />
      </Card>

      {picking && (
        <Modal title="Возврат поставщику: кому" onClose={() => setPicking(false)}>
          <div className="flex flex-col gap-3">
            <ErrorBox error={suppliers.error} />
            <Field label="Поставщик" required>
              <select id="return-supplier" autoFocus value={supplierId} onChange={(e) => setSupplierId(e.target.value)}>
                <option value="">— выберите поставщика —</option>
                {active.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.name}
                  </option>
                ))}
              </select>
            </Field>
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setPicking(false)}>
                Отмена
              </Button>
              <Button
                disabled={!supplierId}
                onClick={() => {
                  setPicking(false)
                  setReturning(active.find((s) => s.id === supplierId) ?? null)
                }}
              >
                Дальше
              </Button>
            </div>
          </div>
        </Modal>
      )}
      {returning && (
        <SupplierReturnModal
          supplierId={returning.id}
          supplierName={returning.name}
          onClose={() => setReturning(null)}
          onDone={() => {
            setReturning(null)
            setListKey((k) => k + 1)
          }}
        />
      )}
    </div>
  )
}
