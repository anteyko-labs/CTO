// Точка входа клиента: монтирует приложение и общие обработчики страницы.
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import App from './App'
import CabinetApp from './pages/cabinet/CabinetApp'
import { shouldSelectOnFocus } from './lib/focus'
import { requestPersistentStorage } from './lib/offline'
import './index.css'

// Числовое поле при фокусе выделяется целиком: ввод заменяет значение, а не дописывается к нему.
// Выделяем, только если поле всё ещё в фокусе и в нём ничего не набрали: select() у поля не в
// фокусе возвращает фокус в него — и набранное дальше уходило в прежнее поле.
document.addEventListener('focusin', (e) => {
  const t = e.target
  if (t instanceof HTMLInputElement && (t.inputMode === 'decimal' || t.inputMode === 'numeric')) {
    const value = t.value
    setTimeout(() => {
      if (shouldSelectOnFocus(document.activeElement === t, value, t.value)) t.select()
    }, 0)
  }
})

// Просим браузер не вычищать хранилище: в нём очередь чеков, ещё не ушедших на сервер (SPEC-09).
// Отказ браузера касса показывает предупреждением (ADR-062).
void requestPersistentStorage()

const root = document.getElementById('root')
if (root) {
  createRoot(root).render(
    <StrictMode>
      {/* Кабинет юрлица живёт отдельно от экранов точки: свой вход, без офлайн-кассы (ADR-049). */}
      {window.location.pathname.startsWith('/cabinet') ? <CabinetApp /> : <App />}
    </StrictMode>,
  )
}
