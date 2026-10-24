// «Все филиалы» в отчётах владельца (SPEC-33 §3): выбор помнит устройство; сервер получает
// `branch=all` и отвечает суммой с разбивкой `branches`.
import { useState } from 'react'

const ALL_KEY = 'avtodom.reports_all'

function readAll(): boolean {
  try {
    return localStorage.getItem(ALL_KEY) === '1'
  } catch {
    return false
  }
}

/** Отчёт одного филиала из разбивки «Все филиалы». */
export interface BranchPart<T> {
  branch_id: string
  name: string
  archived: boolean
  report: T
}

/** Ответ «Все филиалы»: сумма по филиалам и разбивка. */
export type AllBranches<T> = T & { all_branches?: boolean; branches?: BranchPart<T>[] }

/** Сейчас «Все филиалы» (только у владельца при нескольких филиалах — проверяет вызывающий). */
export function useAllBranches(enabled: boolean): [boolean, (v: boolean) => void] {
  const [all, setAll] = useState(readAll)
  const change = (v: boolean) => {
    setAll(v)
    try {
      if (v) localStorage.setItem(ALL_KEY, '1')
      else localStorage.removeItem(ALL_KEY)
    } catch {
      // Без хранилища выбор живёт до перезагрузки.
    }
  }
  return [enabled && all, change]
}

/** Параметр запроса отчёта. */
export const branchParam = (all: boolean): { branch?: string } => (all ? { branch: 'all' } : {})

/** Список строк из ответа: у «Все филиалы» список приходит полем `items`. */
export function rowsOf<T>(v: T[] | { items: T[] }): T[] {
  return Array.isArray(v) ? v : v.items
}
