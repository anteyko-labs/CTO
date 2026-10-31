// Незнакомый код заводят несколько человек сразу (задача 003, Н14): первый заводит товар,
// остальным сервер отвечает 409 «штрихкод … уже у товара» — экран находит этот товар по коду и
// продолжает с ним (прибавляет посчитанное, добавляет в строку), а не показывает ошибку.
import { ApiError, get } from './api'
import type { Product } from './types'

export const TAKEN_ADDED = 'Этот товар только что завёл другой — добавили к нему'
export const TAKEN_EXISTS = 'Этот товар только что завёл другой — продолжаем с ним'

const byBarcode = (code: string) => get<Product>(`/products/by-barcode/${encodeURIComponent(code)}`)

/**
 * Заводит товар; код уже занят (409) и товар с ним находится — возвращает его с `taken`.
 * Без кода, другая ошибка или товар не нашёлся — прежняя ошибка.
 */
export async function createOrTaken<T>(
  code: string,
  create: () => Promise<T>,
  find: (code: string) => Promise<Product> = byBarcode,
): Promise<{ taken: false; created: T } | { taken: true; product: Product }> {
  try {
    return { taken: false, created: await create() }
  } catch (e) {
    if (!code || !(e instanceof ApiError) || e.status !== 409) throw e
    let product: Product
    try {
      product = await find(code)
    } catch {
      throw e
    }
    return { taken: true, product }
  }
}
