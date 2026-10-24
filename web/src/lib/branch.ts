// Филиал запроса (SPEC-33): владелец выбирает филиал в шапке, выбор помнит устройство, сервер
// получает его заголовком `X-Branch-Id`. У администратора выбора нет — сервер всегда отвечает
// по его филиалу.

const BRANCH_KEY = 'avtodom.branch'

let current: string | null = readStored()

function readStored(): string | null {
  try {
    return localStorage.getItem(BRANCH_KEY)
  } catch {
    return null
  }
}

/** Филиал, выбранный на этом устройстве; `null` — свой филиал пользователя. */
export const selectedBranch = (): string | null => current

/** Запомнить выбор (или забыть: `null`). Экраны перечитываются перезагрузкой страницы. */
export function rememberBranch(id: string | null): void {
  current = id
  try {
    if (id) localStorage.setItem(BRANCH_KEY, id)
    else localStorage.removeItem(BRANCH_KEY)
  } catch {
    // Без хранилища выбор живёт до перезагрузки.
  }
}

/** Заголовки запроса с выбранным филиалом. `branch` — филиал записи очереди (чек без сети
 *  уходит в тот филиал, где его пробили, даже если владелец потом переключился). */
export function branchHeaders(branch?: string | null): Record<string, string> {
  const b = branch === undefined ? current : branch
  return b ? { 'X-Branch-Id': b } : {}
}

/** Переключить филиал: запомнить и перечитать экраны (кэши, снимок кассы и счета — филиала). */
export function switchBranch(id: string): void {
  rememberBranch(id)
  window.location.reload()
}

let label: string | null = null

/** Название филиала для печатных документов — когда филиалов несколько (SPEC-33). */
export const branchLabel = (): string | null => label

export function setBranchLabel(name: string | null): void {
  label = name
}
