// Экранная цифровая клавиатура режима кассы (SPEC-29, «Режим кассы»). Появляется у любого
// числового поля с `data-keypad` (количество, литры, «Получено», вес и цена аккумулятора, PIN при
// открытии смены, суммы в окнах) и вводит в него цифры касанием — физическая клавиатура не нужна.
import { useEffect, useLayoutEffect, useReducer, useRef, useState, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent } from 'react'
import { applyKey, padPlace, type KeypadKey, type KeypadKind } from '../lib/keypad'

function isPadField(el: unknown): el is HTMLInputElement {
  return el instanceof HTMLInputElement && el.dataset.keypad !== undefined && !el.disabled && !el.readOnly
}

/** Записать значение в поле так, чтобы React увидел изменение (onChange). */
function setFieldValue(el: HTMLInputElement, value: string): void {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set
  if (setter) setter.call(el, value)
  else el.value = value
  el.dispatchEvent(new Event('input', { bubbles: true }))
}

/** Подпись поля: aria-label или первый текст его подписи (без звёздочки и подсказки). */
function fieldLabel(el: HTMLInputElement): string {
  const aria = el.getAttribute('aria-label')
  if (aria) return aria
  const label = el.labels?.[0]
  if (!label) return ''
  const walker = document.createTreeWalker(label, NodeFilter.SHOW_TEXT)
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    const t = n.textContent?.trim()
    if (t) return t
  }
  return ''
}

/** Ширина клавиатуры, точки: 18rem при шрифте режима 19 px; высота — до первого замера. */
const PAD_WIDTH = 342
const PAD_HEIGHT = 540

const DIGITS: KeypadKey[] = ['7', '8', '9', '4', '5', '6', '1', '2', '3']

// Кнопки клавиатуры не забирают фокус у поля: иначе поле потеряет курсор после каждой цифры.
const keepFocus = (e: ReactPointerEvent | ReactMouseEvent) => e.preventDefault()

export function Keypad() {
  const [target, setTarget] = useState<HTMLInputElement | null>(null)
  const targetRef = useRef<HTMLInputElement | null>(null)
  const padRef = useRef<HTMLDivElement>(null)
  // Поле только что выбрано — первая цифра заменяет подставленное значение.
  const fresh = useRef(true)
  // Фокус вернула сама клавиатура — это не новый выбор поля.
  const refocusing = useRef(false)
  const applying = useRef(false)
  const [, rerender] = useReducer((n: number) => n + 1, 0)

  useEffect(() => {
    const choose = (el: HTMLInputElement | null) => {
      targetRef.current = el
      setTarget(el)
    }
    const onIn = (e: FocusEvent) => {
      if (isPadField(e.target)) {
        if (!refocusing.current) fresh.current = true
        refocusing.current = false
        choose(e.target)
      } else if (!padRef.current?.contains(e.target as Node)) {
        choose(null)
      }
    }
    const onOut = () =>
      setTimeout(() => {
        const a = document.activeElement
        if (isPadField(a) || padRef.current?.contains(a)) return
        // Поле осталось на экране и в фокусе ничего нет (касание по пустому месту) — клавиатуру прячем.
        choose(null)
      }, 0)
    // Набрали с физической клавиатуры — дальше дописываем, значение уже не «подставленное».
    const onInput = (e: Event) => {
      if (e.target === targetRef.current) {
        if (!applying.current) fresh.current = false
        rerender()
      }
    }
    document.addEventListener('focusin', onIn)
    document.addEventListener('focusout', onOut)
    document.addEventListener('input', onInput)
    // Поле уже в фокусе, когда клавиатура появилась (включили режим с курсором в поле).
    if (isPadField(document.activeElement)) choose(document.activeElement)
    return () => {
      document.removeEventListener('focusin', onIn)
      document.removeEventListener('focusout', onOut)
      document.removeEventListener('input', onInput)
    }
  }, [])

  // Размер клавиатуры — замером после показа (с быстрыми суммами она выше).
  const [padSize, setPadSize] = useState({ width: PAD_WIDTH, height: PAD_HEIGHT })
  useLayoutEffect(() => {
    const el = padRef.current
    if (!target || !el) return
    const measure = () => {
      const r = el.getBoundingClientRect()
      setPadSize((cur) => (Math.abs(r.width - cur.width) > 1 || Math.abs(r.height - cur.height) > 1 ? { width: r.width, height: r.height } : cur))
    }
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    return () => ro.disconnect()
  }, [target])
  // Прокрутка и поворот экрана двигают поле — клавиатура идёт за ним.
  useEffect(() => {
    if (!target) return
    const move = () => rerender()
    window.addEventListener('scroll', move, true)
    window.addEventListener('resize', move)
    return () => {
      window.removeEventListener('scroll', move, true)
      window.removeEventListener('resize', move)
    }
  }, [target])
  // Узкий экран: места рядом с полем нет — клавиатура внизу, а страница прокручивается, чтобы
  // поле было над ней. Запас снизу — на время показа, чтобы было куда прокрутить.
  const scrolledFor = useRef<HTMLInputElement | null>(null)
  useEffect(() => {
    if (!target || scrolledFor.current === target) return
    const t = setTimeout(() => {
      if (!target.isConnected) return
      scrolledFor.current = target
      const dialog = target.closest('[role="dialog"]')?.getBoundingClientRect()
      const { scroll } = padPlace(target.getBoundingClientRect(), padSize, { width: window.innerWidth, height: window.innerHeight }, dialog)
      if (scroll <= 0) return
      const root = document.documentElement
      root.style.paddingBottom = `${padSize.height + 24}px`
      const box = target.closest('[role="dialog"] .overflow-y-auto, [role="dialog"]')
      if (box instanceof HTMLElement && box.scrollHeight > box.clientHeight) box.scrollBy({ top: scroll })
      else window.scrollBy({ top: scroll })
    }, 0)
    return () => clearTimeout(t)
  }, [target, padSize])
  useEffect(() => {
    if (target) return
    scrolledFor.current = null
    document.documentElement.style.paddingBottom = ''
  }, [target])
  useEffect(
    () => () => {
      document.documentElement.style.paddingBottom = ''
    },
    [],
  )

  // Поле исчезло (строку удалили, окно закрыли) — клавиатура тоже.
  useEffect(() => {
    if (!target) return
    const t = setInterval(() => {
      if (!target.isConnected) {
        targetRef.current = null
        setTarget(null)
      }
    }, 300)
    return () => clearInterval(t)
  }, [target])

  if (!target || !target.isConnected) return null

  const kind: KeypadKind = target.dataset.keypad === 'decimal' ? 'decimal' : target.dataset.keypad === 'digits' ? 'digits' : 'int'
  const decimals = Number(target.dataset.keypadDecimals) || 2
  const quick = (target.dataset.keypadQuick ?? '').split(';').filter(Boolean)
  // Рядом с полем и не поверх него, целиком на экране (поле в окне — сбоку от окна).
  const dialog = target.closest('[role="dialog"]')?.getBoundingClientRect()
  const place = padPlace(target.getBoundingClientRect(), padSize, { width: window.innerWidth, height: window.innerHeight }, dialog)

  const write = (value: string) => {
    applying.current = true
    setFieldValue(target, value)
    applying.current = false
    fresh.current = false
    if (document.activeElement !== target) {
      refocusing.current = true
      target.focus()
    }
    rerender()
  }
  const press = (key: KeypadKey) => write(applyKey(target.value, key, { kind, fresh: fresh.current, decimals, maxLength: target.maxLength }))
  const done = () => {
    target.blur()
    targetRef.current = null
    setTarget(null)
  }

  const base = 'flex min-h-16 items-center justify-center rounded-lg border font-semibold shadow-sm'
  const key = `${base} border-slate-300 bg-white text-2xl text-slate-900 active:bg-sky-100 disabled:text-slate-300`
  return (
    <div
      ref={padRef}
      role="group"
      aria-label="Цифровая клавиатура"
      data-testid="keypad"
      className="no-print fixed z-50 max-h-[calc(100dvh-1.5rem)] w-[min(18rem,calc(100vw-1.5rem))] overflow-y-auto rounded-xl border border-slate-300 bg-slate-100 p-3 shadow-2xl"
      style={{ left: place.left, top: place.top }}
      onPointerDown={keepFocus}
      onMouseDown={keepFocus}
    >
      <div className="mb-2 flex items-center justify-between gap-2">
        <div className="min-w-0">
          <div className="truncate text-sm text-slate-600">{fieldLabel(target) || 'Ввод числа'}</div>
          <div className="truncate text-2xl font-bold tabular-nums" data-testid="keypad-value">
            {target.type === 'password' ? '•'.repeat(target.value.length) : target.value || '—'}
          </div>
        </div>
      </div>
      {quick.length > 0 && (
        <div className="mb-2 grid grid-cols-3 gap-2">
          {quick.slice(0, 3).map((q) => (
            <button key={q} type="button" className={`${base} border-sky-300 bg-sky-50 text-xl text-sky-900 active:bg-sky-100`} onClick={() => write(q)}>
              {q}
            </button>
          ))}
        </div>
      )}
      <div className="grid grid-cols-3 gap-2">
        {DIGITS.map((d) => (
          <button key={d} type="button" className={key} onClick={() => press(d)}>
            {d}
          </button>
        ))}
        <button type="button" className={key} disabled={kind !== 'decimal'} aria-label="Запятая" onClick={() => press(',')}>
          ,
        </button>
        <button type="button" className={key} onClick={() => press('0')}>
          0
        </button>
        <button type="button" className={key} aria-label="Стереть" onClick={() => press('back')}>
          ⌫
        </button>
        <button type="button" className={`${base} border-slate-300 bg-white text-xl text-rose-700 active:bg-rose-50`} aria-label="Очистить" onClick={() => press('clear')}>
          C
        </button>
        <button type="button" className={`${base} col-span-2 border-sky-700 bg-sky-600 text-xl text-white active:bg-sky-700`} onClick={done}>
          Готово
        </button>
      </div>
    </div>
  )
}
