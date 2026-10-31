import { useState } from 'react'
import { patch } from '../lib/api'
import { useAction } from '../lib/hooks'
import type { Supplier } from '../lib/types'
import { dueDaysOk, innOk, supplierBody, supplierForm, type SupplierForm } from '../lib/supplierForm'
import { Button, Checkbox, ErrorBox, Field, Missing, Modal } from './ui'
import { missingWithFocus } from '../lib/forms'

/** Поля поставщика — одни для нового и для правки. */
export function SupplierFields({ form, setForm, nameId }: { form: SupplierForm; setForm: (f: SupplierForm) => void; nameId: string }) {
  return (
    <>
      <Field label="Название" required>
        <input id={nameId} autoFocus value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} />
      </Field>
      <div className="flex flex-col gap-1 text-sm">
        <span className="font-medium text-slate-700">Вид</span>
        <div className="inline-grid w-fit grid-cols-2 overflow-hidden rounded-md border border-slate-300" role="group" aria-label="Вид поставщика">
          {(['company', 'person'] as const).map((k) => (
            <button
              key={k}
              type="button"
              aria-pressed={form.kind === k}
              className={`min-h-[44px] px-4 ${form.kind === k ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
              onClick={() => setForm({ ...form, kind: k })}
            >
              {k === 'company' ? 'Фирма' : 'Человек'}
            </button>
          ))}
        </div>
      </div>
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label="ИНН" error={innOk(form.inn) ? undefined : 'Только цифры'}>
          <input id="supplier-inn" inputMode="numeric" value={form.inn} onChange={(e) => setForm({ ...form, inn: e.target.value })} />
        </Field>
        <Field label="Телефон">
          <input type="tel" value={form.phone} onChange={(e) => setForm({ ...form, phone: e.target.value })} />
        </Field>
      </div>
      <Field label="Адрес">
        <input id="supplier-address" value={form.address} onChange={(e) => setForm({ ...form, address: e.target.value })} />
      </Field>
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label="Банк" hint="Название и БИК">
          <input id="supplier-bank" value={form.bank} onChange={(e) => setForm({ ...form, bank: e.target.value })} />
        </Field>
        <Field label="Расчётный счёт">
          <input
            id="supplier-account"
            inputMode="numeric"
            value={form.bank_account}
            onChange={(e) => setForm({ ...form, bank_account: e.target.value })}
          />
        </Field>
      </div>
      <Field label="Срок оплаты, дней" hint="Сколько дней поставщик ждёт оплату; пусто — не задан" error={dueDaysOk(form.due_days) ? undefined : 'Целое число дней'}>
        <input
          id="supplier-due"
          className="w-28"
          inputMode="numeric"
          value={form.due_days}
          onChange={(e) => setForm({ ...form, due_days: e.target.value })}
        />
      </Field>
      <Field label="Комментарий">
        <textarea rows={2} value={form.comment} onChange={(e) => setForm({ ...form, comment: e.target.value })} />
      </Field>
    </>
  )
}

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
  const [form, setForm] = useState(() => supplierForm(supplier))
  const [active, setActive] = useState(supplier.active)
  const { busy, error, run } = useAction()
  const notFilled = missingWithFocus([Boolean(form.name.trim()), 'название', '#supplier-edit-name'])
  const valid = innOk(form.inn) && dueDaysOk(form.due_days)

  const save = () =>
    run(async () => {
      await patch<Supplier>(`/suppliers/${supplier.id}`, { ...supplierBody(form, true), active })
      onSaved()
    })

  return (
    <Modal title="Поставщик" onClose={onClose}>
      <div className="flex flex-col gap-3">
        <SupplierFields form={form} setForm={setForm} nameId="supplier-edit-name" />
        <Checkbox label="Активен" checked={active} onChange={setActive} />
        <Missing items={notFilled} />
        <ErrorBox error={error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={busy || notFilled.length > 0 || !valid} onClick={() => void save()}>
            Сохранить
          </Button>
        </div>
      </div>
    </Modal>
  )
}
