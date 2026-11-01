// Привязать штрихкод к заведённому товару (ADR-026, задача 005 Н3). Сервер отвечает:
// — 409 `umag_measure_code` — это код литра (кг, метра) из Умага: экран спрашивает и шлёт снова с `measure_ok`;
// — 409 «штрихкод … уже у товара» — код у этого же товара (повтор, двойное нажатие) — это успех;
//   у другого товара — экран предлагает продолжить с ним (как lib/barcodeTaken.ts).
import { ApiError, get, newOpId, post } from './api'
import type { Product } from './types'

export type LinkResult =
  | { kind: 'linked'; product: Product }
  | { kind: 'taken'; product: Product }
  | { kind: 'measure'; message: string }

export interface LinkDeps {
  send: (productId: string, body: Record<string, unknown>) => Promise<unknown>
  product: (id: string) => Promise<Product>
  byBarcode: (code: string) => Promise<Product>
}

const DEPS: LinkDeps = {
  send: (id, body) => post(`/products/${id}/barcodes`, body),
  product: (id) => get<Product>(`/products/${id}`),
  byBarcode: (code) => get<Product>(`/products/by-barcode/${encodeURIComponent(code)}`),
}

/** Тело запроса привязки: `op_id` (I8), код (нет — сервер выдаст свой) и подтверждение кода меры. */
export function linkBody(code: string | undefined, measureOk = false): Record<string, unknown> {
  return { op_id: newOpId(), ...(code ? { code } : {}), ...(measureOk ? { measure_ok: true } : {}) }
}

/** Привязать код `code` к товару `productId` и вернуть товар с кодом; занятый код — чей он. */
export async function linkCode(productId: string, code: string, measureOk = false, deps: LinkDeps = DEPS): Promise<LinkResult> {
  try {
    await deps.send(productId, linkBody(code, measureOk))
  } catch (e) {
    if (!(e instanceof ApiError) || e.status !== 409) throw e
    if (e.code === 'umag_measure_code') return { kind: 'measure', message: e.message }
    let owner: Product
    try {
      owner = await deps.byBarcode(code)
    } catch {
      throw e
    }
    return owner.id === productId ? { kind: 'linked', product: owner } : { kind: 'taken', product: owner }
  }
  return { kind: 'linked', product: await deps.product(productId) }
}

/** Привязка из карточки и «Сканера товаров»: код меры Умага — переспросить и привязать по согласию.
 *  `false` — человек передумал. */
export async function postBarcodeAsking<T>(productId: string, code: string | undefined, ask: (message: string) => boolean = (m) => window.confirm(m)): Promise<T | false> {
  try {
    return await post<T>(`/products/${productId}/barcodes`, linkBody(code))
  } catch (e) {
    if (!(e instanceof ApiError) || e.code !== 'umag_measure_code') throw e
    if (!ask(e.message)) return false
    return post<T>(`/products/${productId}/barcodes`, linkBody(code, true))
  }
}
