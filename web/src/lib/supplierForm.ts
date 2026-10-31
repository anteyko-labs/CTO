// Форма поставщика с реквизитами (задача 003 Н10): поля, проверка и тело запроса.
import type { Supplier } from './types'

/** Поля формы поставщика: имя, телефон, комментарий и реквизиты (задача 003 Н10). */
export interface SupplierForm {
  name: string
  phone: string
  comment: string
  kind: 'company' | 'person'
  inn: string
  address: string
  bank: string
  bank_account: string
  /** Срок оплаты в днях; пусто — не задан. */
  due_days: string
}

export const EMPTY_SUPPLIER: SupplierForm = {
  name: '',
  phone: '',
  comment: '',
  kind: 'company',
  inn: '',
  address: '',
  bank: '',
  bank_account: '',
  due_days: '',
}

export function supplierForm(s: Supplier): SupplierForm {
  return {
    name: s.name,
    phone: s.phone,
    comment: s.comment,
    kind: s.kind ?? 'company',
    inn: s.inn ?? '',
    address: s.address ?? '',
    bank: s.bank ?? '',
    bank_account: s.bank_account ?? '',
    due_days: s.due_days == null ? '' : String(s.due_days),
  }
}

/** Срок оплаты: пусто или целое число дней от 0 до 3650. */
export function dueDaysOk(v: string): boolean {
  const t = v.trim()
  return t === '' || (/^\d{1,4}$/.test(t) && Number(t) <= 3650)
}

/** ИНН: пусто или только цифры, не больше 20. */
export function innOk(v: string): boolean {
  return /^\d{0,20}$/.test(v.trim())
}

/** Тело запроса: реквизиты как есть, срок — числом; пустой срок при правке снимает его. */
export function supplierBody(f: SupplierForm, editing: boolean): Record<string, unknown> {
  const due = f.due_days.trim()
  return {
    name: f.name,
    phone: f.phone,
    comment: f.comment,
    kind: f.kind,
    inn: f.inn.trim(),
    address: f.address,
    bank: f.bank,
    bank_account: f.bank_account,
    ...(due !== '' ? { due_days: Number(due) } : editing ? { clear_due_days: true } : {}),
  }
}
