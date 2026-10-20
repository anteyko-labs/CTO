// Значок «?» с подсказкой: длинное пояснение не растягивает форму. Нажатие раскрывает текст под
// значком — на сенсоре и телефоне наведения нет (ui-rules §9); повторное нажатие или Esc скрывает.
import { useState } from 'react'

export function DirHint({ text }: { text: string }) {
  const [open, setOpen] = useState(false)
  return (
    <>
      <button
        type="button"
        aria-label={open ? 'Скрыть подсказку' : `Подсказка: ${text}`}
        aria-expanded={open}
        title={text}
        className={`relative ml-1 inline-flex h-6 w-6 items-center justify-center rounded-full border align-middle text-xs font-semibold after:absolute after:-inset-2.5 after:content-[''] ${open ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white text-slate-500 hover:border-sky-400'}`}
        onClick={(e) => {
          e.preventDefault()
          setOpen(!open)
        }}
        onKeyDown={(e) => {
          if (e.key === 'Escape' && open) {
            e.stopPropagation()
            setOpen(false)
          }
        }}
      >
        ?
      </button>
      {open && (
        <span role="note" className="mt-1 block max-w-md rounded-md bg-sky-50 px-2 py-1.5 text-xs font-normal leading-snug text-slate-700">
          {text}
        </span>
      )}
    </>
  )
}
