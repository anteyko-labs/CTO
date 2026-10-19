// Исправление чека владельцем (SPEC-29 §24): исходный чек с сервера → состояние экрана кассы.
// Касса та же, что для нового чека; здесь только перевод строк и оплат в её поля.
import { litersInput, somInput } from './format'
import type { Party, PaymentMethod, Product, SaleLineKind, Service } from './types'

/** Чек в виде запроса проведения — так его отдаёт `GET /corrections/draft/sale/{id}`. */
export interface SaleDraftDoc {
  sale_type: 'takeaway' | 'service'
  cashier_id: string
  master_id: string | null
  party_id: string | null
  contact_id: string | null
  vehicle_id: string | null
  comment: string
  delivery_address: string
  business_date: string
  mileage: number | null
  mileage_unit: string | null
  declined: string
  lines: { kind: SaleLineKind; gift: boolean; product_id: string | null; service_id: string | null; qty: number; unit_price_tyiyn: number }[]
  payments: { method: PaymentMethod; amount_tyiyn: number; grams: number | null; price_per_kg_tyiyn: number | null; account_id: string | null }[]
}

/** Строка чека кассы: количество и цена — текстом, как их правит кассир. */
export interface FixLine {
  key: string
  kind: SaleLineKind
  product?: Product
  service?: Service
  qtyText: string
  priceText: string
  gift?: boolean
}

export type MoneyMethod = 'cash' | 'card' | 'transfer' | 'debt'

export interface SaleFixState {
  saleType: 'takeaway' | 'service'
  cashierId: string
  masterId: string
  lines: FixLine[]
  comment: string
  party: Party | null
  contactId: string
  vehicleId: string
  mileage: string
  delivery: string | null
  declined: string
  payMode: MoneyMethod | 'mixed'
  split: Record<MoneyMethod, string>
  bonusAmount: number
  battery: { kgText: string; perKgText: string } | null
  bankPick: { card: string; transfer: string }
  businessDate: string
}

const MONEY: MoneyMethod[] = ['cash', 'card', 'transfer', 'debt']

/** Количество строки текстом: розлив и вес — литры/килограммы с тысячными, остальное — целым. */
export const qtyInput = (kind: SaleLineKind, qty: number): string => (kind === 'pour' || kind === 'weight' ? litersInput(qty) : String(qty))

/**
 * Состояние кассы из исходного чека. Товары и услуги — карточки с сервера (по id); не нашлась
 * карточка — строка пропускается, кассир увидит, что её нет, и добавит заново.
 */
export function saleFixState(
  doc: SaleDraftDoc,
  products: Map<string, Product>,
  services: Map<string, Service>,
  party: Party | null,
  nextKey: () => string,
): SaleFixState {
  const lines: FixLine[] = []
  for (const l of doc.lines) {
    const product = l.product_id ? products.get(l.product_id) : undefined
    const service = l.service_id ? services.get(l.service_id) : undefined
    if (!product && !service) continue
    lines.push({
      key: nextKey(),
      kind: l.kind,
      product,
      service,
      qtyText: qtyInput(l.kind, l.qty),
      priceText: somInput(l.unit_price_tyiyn),
      gift: l.gift || undefined,
    })
  }
  // Деньги: один способ — он и выбран; несколько — «Смешанная» с суммами по способам.
  const money = doc.payments.filter((p): p is SaleDraftDoc['payments'][number] & { method: MoneyMethod } => MONEY.includes(p.method as MoneyMethod))
  const sums: Record<MoneyMethod, number> = { cash: 0, card: 0, transfer: 0, debt: 0 }
  for (const p of money) sums[p.method] += p.amount_tyiyn
  const used = MONEY.filter((m) => sums[m] > 0)
  const split: Record<MoneyMethod, string> = { cash: '', card: '', transfer: '', debt: '' }
  if (used.length > 1) for (const m of used) split[m] = somInput(sums[m])
  const bank = (m: 'card' | 'transfer') => money.find((p) => p.method === m && p.account_id)?.account_id ?? ''
  const battery = doc.payments.find((p) => p.method === 'battery')
  return {
    saleType: doc.sale_type,
    cashierId: doc.cashier_id,
    masterId: doc.master_id ?? '',
    lines,
    comment: doc.comment,
    party,
    contactId: doc.contact_id ?? '',
    vehicleId: doc.vehicle_id ?? '',
    mileage: doc.mileage != null ? String(doc.mileage) : '',
    delivery: doc.delivery_address ? doc.delivery_address : null,
    declined: doc.declined,
    payMode: used.length > 1 ? 'mixed' : (used[0] ?? 'cash'),
    split,
    bonusAmount: doc.payments.filter((p) => p.method === 'bonus').reduce((acc, p) => acc + p.amount_tyiyn, 0),
    battery:
      battery && battery.grams
        ? { kgText: litersInput(battery.grams), perKgText: battery.price_per_kg_tyiyn != null ? somInput(battery.price_per_kg_tyiyn) : '' }
        : null,
    bankPick: { card: bank('card'), transfer: bank('transfer') },
    businessDate: doc.business_date,
  }
}
