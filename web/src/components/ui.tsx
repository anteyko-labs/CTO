// Общие элементы интерфейса: кнопки, поля, карточки, окна, таблицы (на телефоне — карточками), уведомления.
// Правила поведения интерфейса — docs/tier-3/ui-rules.md.
import { useEffect, useLayoutEffect, useRef, useState, type ButtonHTMLAttributes, type ReactNode } from 'react'
import { focusField } from '../lib/forms'
import { formatSom } from '../lib/format'
import { Icon, type IconName } from './icons'

// Иерархия кнопок: одна главная (primary) на экран, остальные — рамкой; опасные — красной рамкой
// (`danger-outline`) или в меню «⋯»; залитая красная — только подтверждение в окне.
type Variant = 'primary' | 'secondary' | 'danger' | 'danger-outline' | 'ghost'

const VARIANTS: Record<Variant, string> = {
  primary: 'bg-sky-600 text-white hover:bg-sky-700 disabled:bg-slate-200 disabled:text-slate-500',
  secondary: 'bg-white text-slate-800 border border-slate-300 hover:bg-slate-50 disabled:text-slate-400',
  danger: 'bg-rose-600 text-white hover:bg-rose-700 disabled:bg-rose-300',
  'danger-outline': 'bg-white text-rose-700 border border-rose-300 hover:bg-rose-50 disabled:text-rose-300',
  ghost: 'text-slate-700 hover:bg-slate-200 disabled:text-slate-400',
}

export function Button({
  variant = 'primary',
  className = '',
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant }) {
  return (
    <button
      type="button"
      {...props}
      className={`inline-flex min-h-[42px] items-center justify-center gap-2 rounded-md px-4 py-2 text-sm font-medium transition disabled:cursor-not-allowed ${VARIANTS[variant]} ${className}`}
    />
  )
}

/**
 * Поле формы. `required` ставит звёздочку, `error` — красную строку под полем
 * и подсветку рамки (docs/tier-3/ui-rules.md).
 */
export function Field({
  label,
  children,
  hint,
  required = false,
  error,
}: {
  label: string
  children: ReactNode
  hint?: ReactNode
  required?: boolean
  error?: string | null
}) {
  return (
    <label className={`flex flex-col gap-1 text-sm ${error ? '[&_input]:border-rose-400 [&_select]:border-rose-400' : ''}`}>
      <span className="font-medium text-slate-700">
        {label}
        {required && (
          <span className="ml-0.5 text-rose-600" title="Обязательное поле">
            *
          </span>
        )}
      </span>
      {children}
      {error ? (
        <span className="text-xs text-rose-700">{error}</span>
      ) : (
        hint && <span className="text-xs text-slate-500">{hint}</span>
      )}
    </label>
  )
}

/**
 * Чего не хватает для проведения. Показывается рядом с главной кнопкой,
 * пункт можно нажать — фокус перейдёт на поле.
 */
export function Missing({ items, className = '' }: { items: (string | { label: string; focus?: string })[]; className?: string }) {
  if (items.length === 0) return null
  const list = items.map((it) => (typeof it === 'string' ? { label: it, focus: undefined } : it))
  return (
    <div role="status" className={`rounded-md bg-amber-50 px-3 py-2 text-sm text-amber-800 ${className}`}>
      Заполните:{' '}
      {list.map((it, i) => (
        <span key={it.label}>
          {i > 0 && ', '}
          {it.focus ? (
            <button type="button" className="underline decoration-dotted hover:text-amber-900" onClick={() => focusField(it.focus as string)}>
              {it.label}
            </button>
          ) : (
            it.label
          )}
        </span>
      ))}
    </div>
  )
}

export function Card({ children, className = '' }: { children: ReactNode; className?: string }) {
  return <div className={`rounded-lg border border-slate-200 bg-white p-4 shadow-sm ${className}`}>{children}</div>
}

export function PageHeader({ title, actions, back, subtitle }: { title: string; actions?: ReactNode; back?: ReactNode; subtitle?: ReactNode }) {
  return (
    <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
      <div className="min-w-0">
        {back && <div className="mb-1 text-sm">{back}</div>}
        <h1 className="text-xl font-semibold text-balance">{title}</h1>
        {subtitle && <div className="mt-0.5 text-sm text-slate-500">{subtitle}</div>}
      </div>
      {actions && <div className="flex flex-wrap gap-2">{actions}</div>}
    </div>
  )
}

/** Заголовок карточки — один вид на всех экранах. */
export function CardTitle({ children, actions }: { children: ReactNode; actions?: ReactNode }) {
  return (
    <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
      <h2 className="text-base font-semibold">{children}</h2>
      {actions}
    </div>
  )
}

/**
 * Сумма денег: ровные цифры (tabular-nums), настоящий минус, без «,00» для целых сомов.
 * `tone="sign"` — минус красным, плюс зелёным; `zero="—"` — ноль прочерком.
 */
export function Money({
  value,
  tone = 'plain',
  zero,
  className = '',
}: {
  value: number
  tone?: 'plain' | 'sign' | 'negative'
  zero?: string
  className?: string
}) {
  const color =
    tone === 'sign' ? (value < 0 ? 'text-rose-700' : value > 0 ? 'text-emerald-700' : '') : tone === 'negative' && value < 0 ? 'text-rose-700' : ''
  return <span className={`whitespace-nowrap tabular-nums ${color} ${className}`}>{value === 0 && zero !== undefined ? zero : formatSom(value)}</span>
}

/** Плашка-подсказка внутри экрана: внимание (amber), ошибка (rose), справка (sky). */
export function Notice({ children, tone = 'amber' }: { children: ReactNode; tone?: 'amber' | 'rose' | 'sky' | 'green' }) {
  const tones = {
    amber: 'border-amber-200 bg-amber-50 text-amber-900',
    rose: 'border-rose-200 bg-rose-50 text-rose-800',
    sky: 'border-sky-200 bg-sky-50 text-sky-900',
    green: 'border-emerald-200 bg-emerald-50 text-emerald-900',
  }
  return <div className={`rounded-md border px-3 py-2 text-sm ${tones[tone]}`}>{children}</div>
}

export interface MenuItem {
  label: string
  onClick: () => void
  danger?: boolean
  disabled?: boolean
}

/** Меню «⋯» в строке или шапке: редкие и опасные действия не мешают главному. */
export function RowMenu({ items, label = 'Ещё действия' }: { items: MenuItem[]; label?: string }) {
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!open) return
    const close = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false)
    }
    const esc = (e: KeyboardEvent) => e.key === 'Escape' && setOpen(false)
    document.addEventListener('mousedown', close)
    window.addEventListener('keydown', esc)
    return () => {
      document.removeEventListener('mousedown', close)
      window.removeEventListener('keydown', esc)
    }
  }, [open])
  const visible = items.filter((i) => !i.disabled)
  if (visible.length === 0) return null
  return (
    <div ref={ref} className="relative inline-block text-left">
      <button
        type="button"
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        className="inline-flex h-9 w-9 items-center justify-center rounded-md text-slate-500 hover:bg-slate-100 hover:text-slate-800"
        onClick={() => setOpen((o) => !o)}
      >
        <Icon name="dots" />
      </button>
      {open && (
        <div role="menu" className="absolute right-0 z-30 mt-1 min-w-48 overflow-hidden rounded-md border border-slate-200 bg-white py-1 text-sm shadow-lg">
          {visible.map((it) => (
            <button
              key={it.label}
              type="button"
              role="menuitem"
              className={`block w-full px-3 py-2 text-left hover:bg-slate-50 ${it.danger ? 'text-rose-700' : 'text-slate-800'}`}
              onClick={() => {
                setOpen(false)
                it.onClick()
              }}
            >
              {it.label}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}

export function ErrorBox({ error }: { error: string | null | undefined }) {
  if (!error) return null
  return <div className="rounded-md border border-rose-200 bg-rose-50 px-3 py-2 text-sm text-rose-700">{error}</div>
}

export function Loading() {
  return <div className="py-8 text-center text-sm text-slate-500">Загрузка…</div>
}

/** Пустой экран: что здесь появится и кнопка первого действия. */
export function Empty({ children, action, icon }: { children: ReactNode; action?: ReactNode; icon?: IconName }) {
  return (
    <div className="flex flex-col items-center gap-3 py-8 text-center text-sm text-slate-500">
      {icon && (
        <span className="flex h-12 w-12 items-center justify-center rounded-full bg-slate-100 text-slate-400">
          <Icon name={icon} className="h-6 w-6" />
        </span>
      )}
      <div>{children}</div>
      {action}
    </div>
  )
}

export function Badge({ children, tone = 'slate' }: { children: ReactNode; tone?: 'slate' | 'green' | 'amber' | 'rose' | 'sky' }) {
  const tones = {
    slate: 'bg-slate-100 text-slate-700',
    green: 'bg-emerald-100 text-emerald-800',
    amber: 'bg-amber-100 text-amber-800',
    rose: 'bg-rose-100 text-rose-800',
    sky: 'bg-sky-100 text-sky-800',
  }
  return <span className={`inline-flex rounded px-2 py-0.5 text-xs font-medium ${tones[tone]}`}>{children}</span>
}

export function Modal({
  title,
  onClose,
  children,
  wide = false,
}: {
  title: string
  onClose: () => void
  children: ReactNode
  wide?: boolean
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose()
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])
  return (
    <div className="no-print fixed inset-0 z-40 flex items-start justify-center overflow-y-auto bg-black/50 p-4" onMouseDown={onClose}>
      <div
        className={`mt-8 w-full ${wide ? 'max-w-3xl' : 'max-w-lg'} rounded-lg bg-white shadow-xl`}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between border-b border-slate-200 px-4 py-3">
          <h2 className="font-semibold">{title}</h2>
          <button type="button" className="text-slate-500 hover:text-slate-800" onClick={onClose} aria-label="Закрыть">
            ✕
          </button>
        </div>
        <div className="p-4">{children}</div>
      </div>
    </div>
  )
}

/**
 * Таблица. На узком экране строки показываются карточками: подпись столбца берётся из `head`
 * и проставляется ячейкам атрибутом data-label (см. index.css).
 */
export function Table({ head, children }: { head: ReactNode[]; children: ReactNode }) {
  const ref = useRef<HTMLTableElement>(null)
  const labels = head.map((h) => (typeof h === 'string' ? h : ''))
  useLayoutEffect(() => {
    ref.current?.querySelectorAll('tbody > tr').forEach((tr) => {
      Array.from(tr.children).forEach((td, i) => {
        if (td instanceof HTMLElement) td.dataset.label = labels[i] ?? ''
      })
    })
  })
  return (
    <div className="overflow-x-auto">
      <table ref={ref} className="responsive w-full text-sm">
        <thead>
          <tr className="border-b border-slate-200 text-left text-xs uppercase text-slate-500">
            {head.map((h, i) => (
              <th key={i} className="px-2 py-2 font-medium">
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody className="divide-y divide-slate-100">{children}</tbody>
      </table>
    </div>
  )
}

// ---------- Уведомления ----------

type ToastTone = 'success' | 'error'
interface ToastItem {
  id: number
  text: string
  tone: ToastTone
}

let toastSeq = 0
const listeners = new Set<(items: ToastItem[]) => void>()
let toasts: ToastItem[] = []

function emit() {
  for (const l of listeners) l(toasts)
}

/** Короткое уведомление в углу экрана. */
export function toast(text: string, tone: ToastTone = 'success'): void {
  const item = { id: ++toastSeq, text, tone }
  toasts = [...toasts, item]
  emit()
  setTimeout(() => {
    toasts = toasts.filter((t) => t.id !== item.id)
    emit()
  }, 3000)
}

export function Toaster() {
  const [items, setItems] = useState<ToastItem[]>([])
  useEffect(() => {
    listeners.add(setItems)
    return () => {
      listeners.delete(setItems)
    }
  }, [])
  return (
    <div className="no-print pointer-events-none fixed inset-x-0 bottom-20 z-50 flex flex-col items-center gap-2 px-4 md:bottom-6 md:items-end">
      {items.map((t) => (
        <div
          key={t.id}
          role="status"
          className={`rounded-md px-4 py-2 text-sm font-medium text-white shadow-lg ${t.tone === 'success' ? 'bg-emerald-600' : 'bg-rose-600'}`}
        >
          {t.text}
        </div>
      ))}
    </div>
  )
}

export function Checkbox({ label, checked, onChange }: { label: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <label className="inline-flex items-center gap-2 text-sm">
      <input type="checkbox" className="h-4 w-4" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      {label}
    </label>
  )
}
