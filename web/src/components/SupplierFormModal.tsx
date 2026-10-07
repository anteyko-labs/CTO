import { useState } from 'react'
import { patch } from '../lib/api'
import { useAction } from '../lib/hooks'
import type { Supplier } from '../lib/types'
import { Button, Checkbox, ErrorBox, Field, Missing, Modal } from './ui'
import { missingWithFocus } from '../lib/forms'

/** Правка поставщика. Используется в списке и в карточке. */
export function SupplierEditModal({
  supplier,
  onClose,
  onSaved,
}: {
  supplier: Supplier
  onClose: () => void
  onSaved: () => void
}) {
  const [form, setForm] = useState(supplier)
  const { busy, error, run } = useAction()
  const notFilled = missingWithFocus([Boolean(form.name.trim()), 'название', '#supplier-edit-name'])

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
        <Field label="Название" required>
          <input id="supplier-edit-name" value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} />
        </Field>
        <Field label="Телефон">
          <input type="tel" value={form.phone} onChange={(e) => setForm({ ...form, phone: e.target.value })} />
        </Field>
        <Field label="Комментарий">
          <textarea rows={3} value={form.comment} onChange={(e) => setForm({ ...form, comment: e.target.value })} />
        </Field>
        <Checkbox label="Активен" checked={form.active} onChange={(v) => setForm({ ...form, active: v })} />
        <Missing items={notFilled} />
        <ErrorBox error={error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={busy || notFilled.length > 0} onClick={() => void save()}>
            Сохранить
          </Button>
        </div>
      </div>
    </Modal>
  )
}
