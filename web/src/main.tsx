import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import App from './App'
import './index.css'

// Числовое поле при фокусе выделяется целиком: ввод заменяет значение, а не дописывается к нему.
document.addEventListener('focusin', (e) => {
  const t = e.target
  if (t instanceof HTMLInputElement && (t.inputMode === 'decimal' || t.inputMode === 'numeric')) {
    setTimeout(() => t.select(), 0)
  }
})

const root = document.getElementById('root')
if (root) {
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  )
}
