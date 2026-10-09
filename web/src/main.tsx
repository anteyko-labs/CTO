// Точка входа клиента: монтирует приложение и общие обработчики страницы.
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import App from './App'
import CabinetApp from './pages/cabinet/CabinetApp'
import './index.css'

// Числовое поле при фокусе выделяется целиком: ввод заменяет значение, а не дописывается к нему.
document.addEventListener('focusin', (e) => {
  const t = e.target
  if (t instanceof HTMLInputElement && (t.inputMode === 'decimal' || t.inputMode === 'numeric')) {
    setTimeout(() => t.select(), 0)
  }
})

// Просим браузер не вычищать хранилище: в нём очередь чеков, ещё не ушедших на сервер (SPEC-09).
try {
  void navigator.storage?.persist?.().catch(() => undefined)
} catch {
  // Браузер без Storage API — очередь живёт по общим правилам.
}

const root = document.getElementById('root')
if (root) {
  createRoot(root).render(
    <StrictMode>
      {/* Кабинет юрлица живёт отдельно от экранов точки: свой вход, без офлайн-кассы (ADR-049). */}
      {window.location.pathname.startsWith('/cabinet') ? <CabinetApp /> : <App />}
    </StrictMode>,
  )
}
