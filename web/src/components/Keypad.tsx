// Экранная клавиатура режима кассы (SPEC-29, «Режим кассы»; ADR-101). Сама не появляется: её
// открывает кнопка «⌨ Клавиатура» в полосе режима или маленькая кнопка у поля в фокусе, и она
// остаётся открытой при переходе между полями, пока её не скроют. У числовых полей с `data-keypad`
// (количество, литры, «Получено», вес и цена аккумулятора, PIN, суммы в окнах) — цифровая, у
// текстовых (поиск товара, имя, телефон, госномер, комментарий) — полная: русские и кыргызские
// буквы (ң, ө, ү, ё), латиница, цифры, пробел, стереть, Enter; над ней — куда и что набрано. Физическая клавиатура не нужна.
import { useEffect, useLayoutEffect, useReducer, useRef, useState, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent } from 'react'
import { setKeyboardOpen, setKeyboardTop, useKeyboardOpen } from '../lib/cashierMode'
import {
  KBD_DIGITS,
  KBD_KYRGYZ,
  KBD_LETTERS,
  applyKey,
  eraseBack,
  fieldPad,
  kbdPlace,
  padPlace,
  typeText,
  type FieldPad,
  type KeypadKey,
  type KeypadKind,
  type TextEdit,
} from '../lib/keypad'

type Field = HTMLInputElement | HTMLTextAreaElement

/** Какая клавиатура нужна элементу (поле внутри самой клавиатуры — никакая). */
function padOf(el: unknown): FieldPad {
  if (!(el instanceof HTMLInputElement) && !(el instanceof HTMLTextAreaElement)) return null
  if (el.closest('[data-onscreen-keyboard]')) return null
  return fieldPad(el.tagName, el.getAttribute('type'), el.dataset.keypad, el.disabled || el.readOnly)
}

/** Поле за открытым окном: ни клавиатуры, ни кнопки «⌨» — они легли бы поверх кнопок окна, и касание
 *  края кнопки набрало бы знак в поле за окном. Окно закрыли — клавиатура снова у поля. */
function behindDialog(el: Element | null): boolean {
  if (!el) return false
  const all = document.querySelectorAll('[role="dialog"]')
  const top = all.length > 0 ? all[all.length - 1] : null
  return top !== null && !top.contains(el)
}

/** Записать значение в поле так, чтобы React увидел изменение (onChange). */
function setFieldValue(el: Field, value: string): void {
  const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype
  const setter = Object.getOwnPropertyDescriptor(proto, 'value')?.set
  if (setter) setter.call(el, value)
  else el.value = value
  el.dispatchEvent(new Event('input', { bubbles: true }))
}

/** Курсор поля; у полей без выделения (email, number) — `null`, то есть в конце. */
function selection(el: Field): [number | null, number | null] {
  try {
    return [el.selectionStart, el.selectionEnd]
  } catch {
    return [null, null]
  }
}

function setCaret(el: Field, caret: number): void {
  try {
    el.setSelectionRange(caret, caret)
  } catch {
    // У поля нет курсора — набор всё равно идёт в конец.
  }
}

/** Подпись поля: aria-label, первый текст подписи или подсказка в поле. */
function fieldLabel(el: Field): string {
  const aria = el.getAttribute('aria-label')
  if (aria) return aria
  const label = el.labels?.[0]
  if (label) {
    const walker = document.createTreeWalker(label, NodeFilter.SHOW_TEXT)
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      const t = n.textContent?.trim()
      if (t) return t
    }
  }
  return el.placeholder ?? ''
}

/** Размер до первого замера (цифровая — 18rem × ~540); дальше — замером. */
const PAD_SIZE = { width: 342, height: 540 }
/** Кнопка «⌨» у поля, точки. */
const FIELD_BTN = 52

const DIGITS: KeypadKey[] = ['7', '8', '9', '4', '5', '6', '1', '2', '3']

// Кнопки клавиатуры не забирают фокус у поля: иначе поле потеряет курсор после каждой клавиши.
const keepFocus = (e: ReactPointerEvent | ReactMouseEvent) => e.preventDefault()

export function Keypad() {
  const open = useKeyboardOpen()
  // Поле, куда идёт ввод: последнее выбранное, пока оно на экране.
  const [target, setTarget] = useState<Field | null>(null)
  const targetRef = useRef<Field | null>(null)
  // В фокусе ли сейчас поле — тогда у закрытой клавиатуры рядом с ним кнопка «⌨».
  const [focused, setFocused] = useState(false)
  const padRef = useRef<HTMLDivElement>(null)
  // Поле только что выбрано — первая цифра заменяет подставленное значение.
  const fresh = useRef(true)
  // Фокус вернула сама клавиатура — это не новый выбор поля.
  const refocusing = useRef(false)
  const applying = useRef(false)
  const [lang, setLang] = useState<'ru' | 'en'>('ru')
  const [shift, setShift] = useState(false)
  const [, rerender] = useReducer((n: number) => n + 1, 0)

  useEffect(() => {
    const choose = (el: Field | null) => {
      targetRef.current = el
      setTarget(el)
    }
    const onIn = (e: FocusEvent) => {
      if (padOf(e.target)) {
        if (!refocusing.current) fresh.current = true
        refocusing.current = false
        choose(e.target as Field)
        setFocused(true)
      } else if (!padRef.current?.contains(e.target as Node)) {
        // Кнопка, список выбора: ввод по-прежнему в последнее поле, но кнопка «⌨» у него не нужна.
        setFocused(false)
      }
    }
    const onOut = () =>
      setTimeout(() => {
        const a = document.activeElement
        if (padOf(a) || padRef.current?.contains(a)) return
        setFocused(false)
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
    if (padOf(document.activeElement)) {
      choose(document.activeElement as Field)
      setFocused(true)
    }
    return () => {
      document.removeEventListener('focusin', onIn)
      document.removeEventListener('focusout', onOut)
      document.removeEventListener('input', onInput)
      // Вышли из режима кассы — в следующий раз клавиатура снова закрыта.
      setKeyboardOpen(false)
    }
  }, [])

  // Открыли кнопкой в полосе, а поле не выбрано: ввод — в последнее поле или в поиск товара.
  useEffect(() => {
    if (!open) return
    if (padOf(document.activeElement)) return
    const last = targetRef.current
    const next = last?.isConnected ? last : document.querySelector<HTMLInputElement>('[data-picker]')
    if (next && padOf(next)) {
      refocusing.current = last === next
      next.focus()
    }
  }, [open])

  const kind: FieldPad = target ? padOf(target) : null
  // Пока открыта полная клавиатура, системная у текстового поля не выезжает поверх неё.
  useEffect(() => {
    if (!open || !target || kind !== 'letters') return
    const prev = target.getAttribute('inputmode')
    target.setAttribute('inputmode', 'none')
    return () => {
      if (prev === null) target.removeAttribute('inputmode')
      else target.setAttribute('inputmode', prev)
    }
  }, [open, target, kind])

  // Поле за открытым окном — клавиатура и «⌨» не рисуются (окно открылось в той же отрисовке —
  // проверяем ещё раз после неё, до показа на экране).
  const covered = behindDialog(target)
  const coveredDrawn = useRef(covered)
  coveredDrawn.current = covered
  useLayoutEffect(() => {
    if (behindDialog(target) !== coveredDrawn.current) rerender()
  }, [target, open, focused])

  // Размер клавиатуры — замером после показа (с быстрыми суммами цифровая выше).
  const [padSize, setPadSize] = useState(PAD_SIZE)
  const shown = open && Boolean(target?.isConnected) && !covered
  useLayoutEffect(() => {
    const el = padRef.current
    if (!shown || !el) return
    const measure = () => {
      const r = el.getBoundingClientRect()
      setPadSize((cur) => (Math.abs(r.width - cur.width) > 1 || Math.abs(r.height - cur.height) > 1 ? { width: r.width, height: r.height } : cur))
    }
    measure()
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    return () => ro.disconnect()
  }, [shown, kind])
  // Прокрутка и поворот экрана двигают поле — клавиатура и кнопка «⌨» идут за ним.
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

  // Поле сдвинулось без прокрутки (над ним появилась строка «Отложенные», окно выросло) —
  // кнопка «⌨» и клавиатура тоже переезжают: следим за положением поля каждый кадр. И за его
  // текстом: поле очистила программа (товар выбран — поиск пуст) — ярлык набранного тоже пуст.
  useEffect(() => {
    if (!target || (!open && !focused)) return
    let last = ''
    let frame = 0
    const watch = () => {
      const r = target.getBoundingClientRect()
      // И за окнами: открылось или закрылось окно — клавиатура прячется или возвращается.
      const now = `${r.left},${r.top},${r.width},${r.height},${target.value},${behindDialog(target)}`
      // Первый кадр — тоже: значение поля могли сменить в той же отрисовке, что и фокус.
      if (now !== last) {
        rerender()
        last = now
      }
      frame = requestAnimationFrame(watch)
    }
    frame = requestAnimationFrame(watch)
    return () => cancelAnimationFrame(frame)
  }, [target, open, focused])

  const place = (t: Field) => {
    const view = { width: window.innerWidth, height: window.innerHeight }
    const rect = t.getBoundingClientRect()
    if (padOf(t) === 'letters') return kbdPlace(rect, padSize, view)
    const dialog = t.closest('[role="dialog"]')?.getBoundingClientRect()
    return padPlace(rect, padSize, view, dialog)
  }
  // Узкий экран: места рядом с полем нет — клавиатура внизу, а страница прокручивается, чтобы
  // поле было над ней. Запас снизу — на время показа, чтобы было куда прокрутить.
  const scrolledFor = useRef<Field | null>(null)
  useEffect(() => {
    if (!shown || !target || scrolledFor.current === target) return
    const t = setTimeout(() => {
      if (!target.isConnected) return
      scrolledFor.current = target
      const { scroll } = place(target)
      if (scroll <= 0) return
      const root = document.documentElement
      root.style.paddingBottom = `${padSize.height + 24}px`
      const box = target.closest('[role="dialog"] .overflow-y-auto, [role="dialog"]')
      if (box instanceof HTMLElement && box.scrollHeight > box.clientHeight) box.scrollBy({ top: scroll })
      else window.scrollBy({ top: scroll })
    }, 0)
    return () => clearTimeout(t)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [shown, target, padSize])
  useEffect(() => {
    if (shown) return
    scrolledFor.current = null
    document.documentElement.style.paddingBottom = ''
  }, [shown])
  useEffect(
    () => () => {
      document.documentElement.style.paddingBottom = ''
    },
    [],
  )

  // Поле исчезло (строку удалили, окно закрыли) — ввод больше некуда, ждём выбора поля.
  useEffect(() => {
    if (!target) return
    const t = setInterval(() => {
      if (!target.isConnected) {
        targetRef.current = null
        setTarget(null)
        setFocused(false)
      }
    }, 300)
    return () => clearInterval(t)
  }, [target])

  // Верх клавиатуры букв с ярлыком набранного над ней — списку найденного товара (ProductPicker):
  // он кончается выше, строки не прячутся под клавиатурой.
  useLayoutEffect(() => {
    const el = padRef.current
    if (!shown || kind !== 'letters' || !el) {
      setKeyboardTop(null)
      return
    }
    const label = el.querySelector('[data-testid="keyboard-target"]')
    const top = Math.min(el.getBoundingClientRect().top, label?.getBoundingClientRect().top ?? Infinity)
    setKeyboardTop(Math.round(top))
  })
  useEffect(() => () => setKeyboardTop(null), [])

  const hide = () => {
    setKeyboardOpen(false)
    setShift(false)
  }

  // Клавиатура закрыта: у поля в фокусе — маленькая кнопка «⌨», она открывает клавиатуру.
  if (!open) {
    if (!target || !focused || !target.isConnected || covered) return null
    const r = target.getBoundingClientRect()
    if (r.width === 0 && r.height === 0) return null
    // Узкое поле (количество) — кнопка над ним, чтобы не закрыть цифры; широкое — у правого края внутри.
    const narrow = r.width < FIELD_BTN * 3
    const left = Math.min(window.innerWidth - FIELD_BTN - 4, narrow ? r.right - FIELD_BTN : r.right - FIELD_BTN - 4)
    const top = narrow ? (r.top - FIELD_BTN - 4 >= 0 ? r.top - FIELD_BTN - 4 : r.bottom + 4) : r.top + (r.height - FIELD_BTN) / 2
    return (
      <button
        type="button"
        data-onscreen-keyboard
        data-testid="keyboard-field-button"
        aria-label="Клавиатура"
        title="Открыть экранную клавиатуру"
        className="no-print fixed z-50 flex items-center justify-center rounded-lg border border-slate-300 bg-white text-2xl text-slate-700 shadow-md active:bg-sky-100"
        style={{ left, top, width: FIELD_BTN, height: FIELD_BTN, minHeight: FIELD_BTN, minWidth: FIELD_BTN }}
        onPointerDown={keepFocus}
        onMouseDown={keepFocus}
        onClick={() => setKeyboardOpen(true)}
      >
        ⌨
      </button>
    )
  }

  if (target && covered) return null

  // Открыта, а поля нет (окно закрыли): узкая полоса, ввод появится, когда выберут поле.
  if (!target || !target.isConnected || !kind) {
    return (
      <div
        ref={padRef}
        data-onscreen-keyboard
        role="group"
        aria-label="Экранная клавиатура"
        data-testid="keyboard-idle"
        className="no-print fixed bottom-3 left-1/2 z-50 flex -translate-x-1/2 items-center gap-4 rounded-xl border border-slate-300 bg-slate-100 px-4 py-2 shadow-2xl"
        onPointerDown={keepFocus}
        onMouseDown={keepFocus}
      >
        <span className="text-base text-slate-600">⌨ Нажмите на поле, куда вводить</span>
        <button type="button" className="rounded-lg border border-slate-300 bg-white px-5 font-semibold text-slate-800 active:bg-slate-200" onClick={hide}>
          Скрыть
        </button>
      </div>
    )
  }

  const pos = place(target)

  const write = (value: string, caret?: number) => {
    applying.current = true
    setFieldValue(target, value)
    applying.current = false
    fresh.current = false
    if (document.activeElement !== target) {
      refocusing.current = true
      target.focus()
    }
    if (caret !== undefined) setCaret(target, caret)
    rerender()
  }

  if (kind === 'digits') {
    const numKind: KeypadKind = target.dataset.keypad === 'decimal' ? 'decimal' : target.dataset.keypad === 'digits' ? 'digits' : 'int'
    const decimals = Number(target.dataset.keypadDecimals) || 2
    const quick = (target.dataset.keypadQuick ?? '').split(';').filter(Boolean)
    const press = (key: KeypadKey) =>
      write(applyKey(target.value, key, { kind: numKind, fresh: fresh.current, decimals, maxLength: target instanceof HTMLInputElement ? target.maxLength : 0 }))
    const base = 'flex min-h-16 items-center justify-center rounded-lg border font-semibold shadow-sm'
    const key = `${base} border-slate-300 bg-white text-2xl text-slate-900 active:bg-sky-100 disabled:text-slate-300`
    return (
      <div
        ref={padRef}
        data-onscreen-keyboard
        role="group"
        aria-label="Цифровая клавиатура"
        data-testid="keypad"
        className="no-print fixed z-50 max-h-[calc(100dvh-1.5rem)] w-[min(18rem,calc(100vw-1.5rem))] overflow-y-auto rounded-xl border border-slate-300 bg-slate-100 p-3 shadow-2xl"
        style={{ left: pos.left, top: pos.top }}
        onPointerDown={keepFocus}
        onMouseDown={keepFocus}
      >
        <div className="mb-2 min-w-0">
          <div className="truncate text-sm text-slate-600">{fieldLabel(target) || 'Ввод числа'}</div>
          <div className="truncate text-2xl font-bold tabular-nums" data-testid="keypad-value">
            {target instanceof HTMLInputElement && target.type === 'password' ? '•'.repeat(target.value.length) : target.value || '—'}
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
          <button type="button" className={key} disabled={numKind !== 'decimal'} aria-label="Запятая" onClick={() => press(',')}>
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
          <button type="button" className={`${base} col-span-2 border-sky-700 bg-sky-600 text-xl text-white active:bg-sky-700`} onClick={hide}>
            Скрыть
          </button>
        </div>
      </div>
    )
  }

  // Полная клавиатура: буквы на месте курсора, как с обычной клавиатуры.
  const edit = (e: TextEdit) => write(e.value, e.caret)
  const type = (text: string) => {
    const [a, b] = selection(target)
    edit(typeText(target.value, a, b, text, target.maxLength))
  }
  const letter = (ch: string) => {
    type(shift ? ch.toUpperCase() : ch)
    setShift(false)
  }
  const erase = () => {
    const [a, b] = selection(target)
    edit(eraseBack(target.value, a, b))
  }
  const enter = () => {
    if (target instanceof HTMLTextAreaElement) {
      type('\n')
      return
    }
    // Enter — как с клавиатуры: поиск товара выбирает найденное, формы отправляются.
    const init = { key: 'Enter', code: 'Enter', keyCode: 13, which: 13, bubbles: true, cancelable: true }
    const handled = !target.dispatchEvent(new KeyboardEvent('keydown', init))
    target.dispatchEvent(new KeyboardEvent('keyup', init))
    if (!handled && target.form) target.form.requestSubmit()
  }
  // Ряд из 12 клавиш одной ширины; широкие клавиши — несколько долей.
  const unit = (n: number) => ({ flex: `0 0 calc((100% - 11 * 0.375rem) / 12 * ${n} + ${n - 1} * 0.375rem)` })
  // Клавиши по 56 точек — под палец (ui-rules §9) и без лишней высоты: клавиатура и так в пол-экрана.
  const base = 'flex h-[56px] items-center justify-center rounded-lg border font-semibold shadow-sm select-none'
  const key = `${base} border-slate-300 bg-white text-2xl text-slate-900 active:bg-sky-100`
  const fn = `${base} border-slate-300 bg-slate-200 text-lg text-slate-800 active:bg-slate-300`
  const rows = KBD_LETTERS[lang]
  return (
    <div
      ref={padRef}
      data-onscreen-keyboard
      role="group"
      aria-label={`Экранная клавиатура: ${fieldLabel(target) || 'ввод текста'}`}
      data-testid="keyboard"
      data-lang={lang}
      className="no-print fixed z-50 flex w-[min(64rem,calc(100vw-1.5rem))] flex-col gap-1.5 rounded-xl border border-slate-300 bg-slate-100 p-2 shadow-2xl"
      style={{ left: pos.left, top: pos.top }}
      onPointerDown={keepFocus}
      onMouseDown={keepFocus}
    >
      {/* Куда и что набрано — как у цифровой: поле может быть закрыто клавиатурой. Ярлык снаружи,
          слева над клавиатурой (у верхнего края экрана — под ней): высоту клавиатуры не прибавляет
          и не закрывает правую колонку кассы. */}
      <div
        className={`pointer-events-none absolute left-3 flex max-w-[45%] min-w-0 items-baseline gap-2 rounded-lg bg-slate-800 px-3 py-1 text-white shadow-lg ${
          pos.top < 48 ? 'top-full mt-1' : 'bottom-full mb-1'
        }`}
        data-testid="keyboard-target"
      >
        <span className="max-w-[50%] shrink-0 truncate text-sm text-slate-300">{fieldLabel(target) || 'Ввод текста'}</span>
        <span className="min-w-0 flex-1 truncate text-lg font-semibold" data-testid="keyboard-value">
          {target.value || <span className="text-slate-400">—</span>}
        </span>
      </div>
      <div className="flex justify-center gap-1.5">
        {KBD_DIGITS.map((d) => (
          <button key={d} type="button" className={key} style={unit(1)} onClick={() => type(d)}>
            {d}
          </button>
        ))}
        <button type="button" className={fn} style={unit(1)} aria-label="Стереть" onClick={erase}>
          ⌫
        </button>
      </div>
      {rows.slice(0, 2).map((row) => (
        <div key={row[0]} className="flex justify-center gap-1.5">
          {row.map((ch) => {
            const shownCh = shift ? ch.toUpperCase() : ch
            return (
              <button key={ch} type="button" className={key} style={unit(1)} aria-label={shownCh} onClick={() => letter(ch)}>
                {shownCh}
              </button>
            )
          })}
        </div>
      ))}
      <div className="flex justify-center gap-1.5">
        <button
          type="button"
          aria-label="Заглавная буква"
          aria-pressed={shift}
          className={shift ? `${base} border-sky-600 bg-sky-100 text-2xl text-sky-900` : fn}
          style={unit(1)}
          onClick={() => setShift((s) => !s)}
        >
          ⇧
        </button>
        {rows[2].map((ch) => {
          const shownCh = shift ? ch.toUpperCase() : ch
          return (
            <button key={ch} type="button" className={key} style={unit(1)} aria-label={shownCh} onClick={() => letter(ch)}>
              {shownCh}
            </button>
          )
        })}
        {['.', ','].map((ch) => (
          <button key={ch} type="button" className={key} style={unit(1)} aria-label={ch === '.' ? 'Точка' : 'Запятая'} onClick={() => type(ch)}>
            {ch}
          </button>
        ))}
      </div>
      <div className="flex justify-center gap-1.5">
        <button
          type="button"
          className={fn}
          style={unit(2)}
          aria-label={lang === 'ru' ? 'Латиница' : 'Русские буквы'}
          onClick={() => setLang((l) => (l === 'ru' ? 'en' : 'ru'))}
        >
          {lang === 'ru' ? 'РУС → LAT' : 'LAT → РУС'}
        </button>
        {lang === 'ru' &&
          KBD_KYRGYZ.map((ch) => {
            const shownCh = shift ? ch.toUpperCase() : ch
            return (
              <button key={ch} type="button" className={key} style={unit(1)} aria-label={shownCh} onClick={() => letter(ch)}>
                {shownCh}
              </button>
            )
          })}
        <button type="button" className={key} style={unit(lang === 'ru' ? 3 : 6)} aria-label="Пробел" onClick={() => type(' ')}>
          пробел
        </button>
        <button type="button" className={`${base} border-sky-700 bg-sky-600 text-lg text-white active:bg-sky-700`} style={unit(2)} aria-label="Enter" onClick={enter}>
          Enter ↵
        </button>
        <button type="button" className={fn} style={unit(2)} onClick={hide}>
          Скрыть
        </button>
      </div>
    </div>
  )
}
