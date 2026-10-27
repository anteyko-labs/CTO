// Экраны, кроме кассы и входа, — отдельными частями сборки (SPEC-36 §16): касса открывается без
// экранов владельца. Части лежат в кэше Service Worker вместе с оболочкой, поэтому без сети
// открываются так же (SPEC-09).
import { lazy, type ComponentType } from 'react'

const RELOADED = 'avtodom.chunk_reload'

/** Часть сборки не загрузилась: после обновления сервера старые части удалены — один раз
 *  перезагружаем страницу (чек кассы хранится в sessionStorage и не теряется). */
// eslint-disable-next-line @typescript-eslint/no-explicit-any -- как у самого React.lazy
export function lazyPage<T extends ComponentType<any>>(load: () => Promise<{ default: T }>) {
  return lazy(() =>
    load().then(
      (m) => {
        try {
          sessionStorage.removeItem(RELOADED)
        } catch {
          // Без хранилища — без отметки.
        }
        return m
      },
      (e: unknown) => {
        let again = false
        try {
          again = sessionStorage.getItem(RELOADED) === null
          sessionStorage.setItem(RELOADED, '1')
        } catch {
          again = false
        }
        if (again && navigator.onLine) {
          window.location.reload()
          return new Promise<never>(() => undefined)
        }
        throw e
      },
    ),
  )
}
