// «Приёмка» на планшете (SPEC-39 §3, ADR-134): список «Ждут приёмки», сверка строк накладной без
// цен и сумм, правка строк кассиром, «не по накладной», затем кассир, PIN, подпись — приход
// проводится по принятому одной операцией. Отметки хранит устройство до проведения.
import { useCallback, useEffect, useMemo, useState } from 'react'
import { useNavigate, useParams } from 'react-router-dom'
import { ProductPicker } from '../components/ProductPicker'
import { SignaturePad } from '../components/SignaturePad'
import { Badge, Button, Empty, ErrorBox, Field, Loading, Modal, Notice, PageHeader, toast } from '../components/ui'
import {
  REASONS,
  acceptBody,
  acceptProblems,
  acceptedInput,
  clearMarks,
  diffCount,
  diffText,
  dropStaleMarks,
  inputKind,
  inputUnit,
  lineByCode,
  lineState,
  loadMarks,
  overLines,
  parseAccepted,
  qtyWords,
  saveMarks,
  scanStep,
  type Acceptance as AcceptanceDoc,
  type AcceptanceLine,
  type AcceptanceList,
  type Extra,
  type LineMark,
  type Marks,
  type Reason,
} from '../lib/acceptances'
import { get, newOpId, post } from '../lib/api'
import { enterFullscreen, setAcceptanceMode, setKeyboardOpen, useAcceptanceMode, useNumField, useTouchMode } from '../lib/cashierMode'
import { formatDateTime, plural } from '../lib/format'
import { useAction, useLoad, usePolling, useServerOnline } from '../lib/hooks'
import { SCAN_EVENT, useScanToPicker } from '../lib/scanner'
import type { Employee } from '../lib/types'

const OFFLINE = 'Нет связи с сервером — приёмка недоступна, отметки сохранены на планшете'

export default function Acceptance() {
  const { id } = useParams()
  return id ? <AcceptanceWork key={id} id={id} /> : <AcceptanceWaiting />
}

/** Полоса режима приёмки: название, клавиатура, выход из режима. */
function ModeBar() {
  const mode = useAcceptanceMode()
  const touch = useTouchMode()
  if (!mode) {
    return (
      <Button
        variant="secondary"
        data-testid="acceptance-mode-on"
        onClick={() => {
          setAcceptanceMode(true)
          enterFullscreen()
        }}
      >
        Режим приёмки на этом устройстве
      </Button>
    )
  }
  return (
    <div className="flex flex-wrap gap-2">
      {touch && (
        <Button variant="secondary" onMouseDown={(e) => e.preventDefault()} onClick={() => setKeyboardOpen(true)}>
          ⌨ Клавиатура
        </Button>
      )}
      <Button variant="secondary" data-testid="acceptance-mode-off" onClick={() => setAcceptanceMode(false)}>
        Выйти из режима
      </Button>
    </div>
  )
}

function AcceptanceWaiting() {
  const navigate = useNavigate()
  const online = useServerOnline()
  const list = useLoad(() => get<AcceptanceList>('/acceptances?status=waiting'), [])
  usePolling(list.reload, 10_000)
  // Отметки снятых и принятых приёмок на устройстве больше не нужны.
  useEffect(() => {
    if (list.data) dropStaleMarks(list.data.items.map((i) => i.id))
  }, [list.data])
  const items = list.data?.items ?? []
  return (
    <div className="flex flex-col gap-4">
      <PageHeader title="Приёмка товара" actions={<ModeBar />} />
      {!online && <Notice tone="rose">{OFFLINE}</Notice>}
      <ErrorBox error={list.error} />
      {list.loading && !list.data ? (
        <Loading />
      ) : items.length === 0 ? (
        <Empty icon="incoming">Товар не ждёт приёмки</Empty>
      ) : (
        <ul className="flex flex-col gap-3" data-testid="acceptance-list">
          {items.map((a) => (
            <li key={a.id}>
              <button
                type="button"
                data-testid="acceptance-card"
                className="flex min-h-[88px] w-full flex-col items-start gap-1 rounded-xl border-2 border-sky-300 bg-sky-50 px-5 py-4 text-left active:bg-sky-100"
                onClick={() => navigate(`/acceptance/${a.id}`)}
              >
                <span className="text-xl font-bold">
                  Примите товар: {a.supplier_name}
                  {a.supplier_doc && `, накладная № ${a.supplier_doc}`}
                </span>
                <span className="text-base text-slate-600">
                  {a.lines_count} {plural(a.lines_count, ['строка', 'строки', 'строк'])} · отправил {a.sent_by_name} ·{' '}
                  {formatDateTime(a.sent_at)}
                  {a.from_bot && ' · из бота'}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}

const TONE: Record<string, string> = {
  none: 'border-slate-300 bg-slate-50',
  ok: 'border-emerald-400 bg-emerald-50',
  less: 'border-amber-400 bg-amber-50',
  more: 'border-amber-400 bg-amber-50',
  zero: 'border-amber-400 bg-amber-50',
}

function AcceptanceWork({ id }: { id: string }) {
  const navigate = useNavigate()
  const online = useServerOnline()
  const touch = useTouchMode()
  useScanToPicker(touch)
  const doc = useLoad(() => get<AcceptanceDoc>(`/acceptances/${id}`), [id])
  const [marks, setMarksState] = useState<Marks>(() => loadMarks(id))
  const setMarks = useCallback(
    (fn: (m: Marks) => Marks) =>
      setMarksState((m) => {
        const next = fn(m)
        saveMarks(id, next)
        return next
      }),
    [id],
  )
  const setLine = useCallback(
    (no: number, patch: Partial<LineMark>) =>
      setMarks((m) => ({ ...m, lines: { ...m.lines, [no]: { ...(m.lines[no] ?? { qty: null }), ...patch } } })),
    [setMarks],
  )
  const [editing, setEditing] = useState<AcceptanceLine | null>(null)
  const [extra, setExtra] = useState<Extra | null>(null)
  const [step, setStep] = useState<'none' | 'accept' | 'refuse'>('none')
  const [photo, setPhoto] = useState(false)
  const [scan, setScan] = useState('')
  const a = doc.data
  const lines = useMemo(() => a?.lines ?? [], [a])

  // Сканер: штрихкод товара прибавляет 1 к «Принято» его строки (канистра — одна канистра);
  // товара нет в накладной — «Добавить как «не по накладной»?».
  const onCode = useCallback(
    (code: string) => {
      const l = lineByCode(lines, code)
      if (!l) {
        if (window.confirm(`Товара с кодом ${code} нет в накладной. Добавить как «не по накладной»?`)) setExtra({ name: '', barcode: code, qty: 1 })
        return
      }
      const add = scanStep(l)
      if (add === null) {
        toast(`${l.name}: введите принятое количество`)
        return
      }
      setMarks((m) => {
        const cur = m.lines[l.line_no]
        return { ...m, lines: { ...m.lines, [l.line_no]: { ...cur, qty: (cur?.qty ?? 0) + add } } }
      })
    },
    [lines, setMarks],
  )
  const modalOpen = editing !== null || extra !== null || step !== 'none' || photo
  useEffect(() => {
    if (modalOpen) return
    const el = document.getElementById('acceptance-scan')
    const on = (e: Event) => {
      const code = (e as CustomEvent<string>).detail
      if (typeof code === 'string' && code.trim()) onCode(code.trim())
      setScan('')
    }
    el?.addEventListener(SCAN_EVENT, on)
    return () => el?.removeEventListener(SCAN_EVENT, on)
  }, [onCode, modalOpen, a])

  // Приёмку уже закрыли — отметки на устройстве не нужны.
  useEffect(() => {
    if (a && a.status !== 'waiting') clearMarks(id)
  }, [a, id])

  if (doc.loading && !a) return <Loading />
  if (!a) {
    return (
      <div className="flex flex-col gap-3">
        <ErrorBox error={doc.error} />
        {!online && <Notice tone="rose">{OFFLINE}</Notice>}
        <Button variant="secondary" onClick={() => navigate('/acceptance')}>
          ← К списку
        </Button>
      </div>
    )
  }
  if (a.status !== 'waiting') {
    return (
      <div className="flex flex-col gap-3">
        <Notice tone="sky">
          {a.status === 'accepted' ? `Принято: приход № ${a.receipt_number ?? ''}` : a.status === 'refused' ? 'Отказ от приёмки' : 'Убрано с приёмки'}
        </Notice>
        <Button variant="secondary" onClick={() => navigate('/acceptance')}>
          ← К списку
        </Button>
      </div>
    )
  }
  const problems = acceptProblems(lines, marks)

  return (
    <div className="flex flex-col gap-3 pb-6">
      <PageHeader
        title={`Примите товар: ${a.supplier_name}`}
        subtitle={`${a.supplier_doc ? `Накладная № ${a.supplier_doc} · ` : ''}${a.lines_count} ${plural(a.lines_count, ['строка', 'строки', 'строк'])} · отправил ${a.sent_by_name}`}
        back={
          <Button variant="secondary" onClick={() => navigate('/acceptance')}>
            ← Список
          </Button>
        }
        actions={<ModeBar />}
      />
      {!online && <Notice tone="rose">{OFFLINE}</Notice>}
      <div className="flex flex-wrap items-center gap-3">
        {a.photo_type && (
          <button type="button" className="rounded-lg border border-slate-300 px-4 py-2 text-base" onClick={() => setPhoto(true)} data-testid="acceptance-photo">
            Фото накладной
          </button>
        )}
        <label className="flex min-w-0 flex-1 items-center gap-2">
          <span className="text-sm text-slate-500">Сканер</span>
          <input
            id="acceptance-scan"
            data-picker
            value={scan}
            placeholder="Отсканируйте штрихкод товара"
            className="min-w-0 flex-1"
            onChange={(e) => setScan(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && scan.trim()) {
                onCode(scan.trim())
                setScan('')
              }
            }}
          />
        </label>
      </div>
      {a.comment && <div className="text-sm text-slate-600">Комментарий: {a.comment}</div>}

      <ul className="flex flex-col gap-2" data-testid="acceptance-lines">
        {lines.map((l) => (
          <LineRow key={l.line_no} line={l} mark={marks.lines[l.line_no]} onMark={(p) => setLine(l.line_no, p)} onEdit={() => setEditing(l)} />
        ))}
      </ul>

      <div className="flex flex-col gap-2 rounded-lg border border-slate-200 p-3">
        <div className="flex flex-wrap items-center gap-3">
          <span className="text-lg font-semibold">Не по накладной</span>
          <Button variant="secondary" data-testid="acceptance-extra-add" onClick={() => setExtra({ name: '', barcode: '', qty: 1 })}>
            + Не по накладной
          </Button>
          <span className="text-sm text-slate-500">в приход не входит — владельцу</span>
        </div>
        {marks.extras.map((x, i) => (
          <div key={i} className="flex items-center gap-3 text-base" data-testid="acceptance-extra">
            <span className="flex-1">
              {x.name || x.barcode} — {x.qty} шт
            </span>
            <Button variant="secondary" onClick={() => setMarks((m) => ({ ...m, extras: m.extras.filter((_, j) => j !== i) }))}>
              Убрать
            </Button>
          </div>
        ))}
        <Field label="Комментарий к приёмке">
          <input value={marks.comment} maxLength={500} onChange={(e) => setMarks((m) => ({ ...m, comment: e.target.value }))} />
        </Field>
      </div>

      {problems.length > 0 && <div className="text-base text-amber-800">{problems[0]}</div>}
      <div className="flex flex-wrap justify-between gap-3">
        <Button variant="danger" className="px-6 text-lg" data-testid="acceptance-refuse" disabled={!online} onClick={() => setStep('refuse')}>
          Отказ от приёмки
        </Button>
        <Button
          className="px-10 text-xl"
          data-testid="acceptance-accept"
          disabled={problems.length > 0 || !online}
          onClick={() => {
            const over = overLines(lines, marks)
            if (over.length && !window.confirm(`Привезли больше, чем в накладной: ${over.join('; ')}. Принять?`)) return
            setStep('accept')
          }}
        >
          Принять
        </Button>
      </div>

      {photo && (
        <Modal title="Фото накладной" full onClose={() => setPhoto(false)}>
          {a.photo_type === 'application/pdf' ? (
            <a className="text-sky-700 underline" href={`/api/v1/acceptances/${id}/photo`} target="_blank" rel="noreferrer">
              Открыть PDF
            </a>
          ) : (
            <img src={`/api/v1/acceptances/${id}/photo`} alt="фото накладной" className="mx-auto max-h-[80vh] object-contain" />
          )}
        </Modal>
      )}
      {editing && (
        <EditLine
          line={editing}
          mark={marks.lines[editing.line_no]}
          onClose={() => setEditing(null)}
          onSave={(m) => {
            setLine(editing.line_no, m)
            setEditing(null)
          }}
        />
      )}
      {extra && (
        <ExtraModal
          initial={extra}
          onClose={() => setExtra(null)}
          onSave={(x) => {
            setMarks((m) => ({ ...m, extras: [...m.extras, x].slice(0, 50) }))
            setExtra(null)
          }}
        />
      )}
      {step === 'accept' && (
        <SignSteps
          title="Принять товар"
          summary={`Принять ${lines.length} ${plural(lines.length, ['строку', 'строки', 'строк'])}, расхождений ${diffCount(lines, marks)}${marks.extras.length ? `, не по накладной ${marks.extras.length}` : ''}`}
          onClose={() => setStep('none')}
          submit={async (employee_id, pin, signature) => {
            const r = await post<{ receipt_number: number }>(`/acceptances/${id}/accept`, {
              op_id: opIdFor(id, 'accept'),
              employee_id,
              pin,
              signature_png: signature,
              ...acceptBody(lines, marks),
            })
            clearMarks(id)
            forgetOpId(id, 'accept')
            toast(`Приход № ${r.receipt_number} проведён`)
            navigate('/acceptance')
          }}
        />
      )}
      {step === 'refuse' && (
        <SignSteps
          title="Отказ от приёмки"
          refuse
          summary="Приёмка снимется, накладная вернётся владельцу"
          onClose={() => setStep('none')}
          submit={async (employee_id, pin, _signature, reason) => {
            await post(`/acceptances/${id}/refuse`, { op_id: opIdFor(id, 'refuse'), employee_id, pin, reason })
            clearMarks(id)
            forgetOpId(id, 'refuse')
            toast('Отказ от приёмки записан')
            navigate('/acceptance')
          }}
        />
      )}
    </div>
  )
}

// Идентификатор операции живёт на устройстве до ответа: повтор после обрыва — тот же приход
// (инвариант 4).
function opIdFor(id: string, what: string): string {
  const key = `avtodom.acceptance_op.${what}.${id}`
  try {
    const v = localStorage.getItem(key)
    if (v) return v
    const n = newOpId()
    localStorage.setItem(key, n)
    return n
  } catch {
    return newOpId()
  }
}

function forgetOpId(id: string, what: string): void {
  try {
    localStorage.removeItem(`avtodom.acceptance_op.${what}.${id}`)
  } catch {
    // Нечего убирать.
  }
}

function LineRow({ line: l, mark, onMark, onEdit }: { line: AcceptanceLine; mark: LineMark | undefined; onMark: (m: Partial<LineMark>) => void; onEdit: () => void }) {
  const num = useNumField()
  const state = lineState(l, mark)
  const [text, setText] = useState(() => acceptedInput(l, mark?.qty ?? null))
  // Отметку поменяли не в поле (сканер, «✓», «Изменить») — поле показывает её.
  useEffect(() => {
    const cur = parseAccepted(l, text)
    if (cur !== (mark?.qty ?? null)) setText(acceptedInput(l, mark?.qty ?? null))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mark?.qty])
  const diff = diffText(l, mark)
  return (
    <li className={`flex flex-col gap-2 rounded-xl border-2 px-4 py-3 ${TONE[state]}`} data-testid="acceptance-line" data-state={state}>
      <div className="flex flex-wrap items-center gap-3">
        <div className="min-w-0 flex-1">
          <div className="text-lg font-semibold">{l.name}</div>
          <div className="text-sm text-slate-600">
            {l.article || l.barcodes[0] || ''}
            {(l.article || l.barcodes[0]) && ' · '}
            по накладной: <b>{qtyWords(l, l.invoice_qty)}</b>
            {mark?.product && (
              <span className="ml-2 text-amber-800">
                → принят: {mark.product.name}
              </span>
            )}
          </div>
        </div>
        <Button
          className="min-w-[96px] px-4 text-lg"
          variant={state === 'ok' ? 'primary' : 'secondary'}
          data-testid="acceptance-line-ok"
          onClick={() => {
            setText(acceptedInput(l, l.invoice_qty))
            onMark({ qty: l.invoice_qty, reason: null })
          }}
        >
          ✓ {acceptedInput(l, l.invoice_qty)}
        </Button>
        <label className="flex items-center gap-2">
          <span className="text-sm text-slate-600">Принято</span>
          <input
            {...num(inputKind(l), inputKind(l) === 'decimal' ? { decimals: 3 } : undefined)}
            aria-label={`Принято: ${l.name}`}
            data-testid="acceptance-line-qty"
            className="w-24 text-right text-lg"
            value={text}
            onChange={(e) => {
              setText(e.target.value)
              onMark({ qty: parseAccepted(l, e.target.value) })
            }}
          />
          <span className="text-sm">{inputUnit(l)}</span>
        </label>
        <Button variant="secondary" data-testid="acceptance-line-edit" onClick={onEdit}>
          Изменить
        </Button>
      </div>
      {(diff || mark?.reason || mark?.comment) && (
        <div className="flex flex-wrap gap-2 text-base">
          {diff && <Badge tone="amber">{diff}</Badge>}
          {mark?.reason && mark.qty !== null && mark.qty < l.invoice_qty && mark.qty > 0 && (
            <span className="text-amber-800">{REASONS.find(([k]) => k === mark.reason)?.[1]}</span>
          )}
          {mark?.comment && <span className="text-slate-600">«{mark.comment}»</span>}
        </div>
      )}
      {mark && mark.qty !== null && mark.qty < l.invoice_qty && !mark.reason && (
        <div className="flex flex-wrap items-center gap-2" data-testid="acceptance-line-reasons">
          <span className="text-sm text-amber-800">Причина:</span>
          {REASONS.map(([k, label]) => (
            <Button key={k} variant="secondary" onClick={() => onMark({ reason: k })}>
              {label}
            </Button>
          ))}
        </div>
      )}
    </li>
  )
}

/** «Изменить» строку: принято, другой товар той же единицы, причина, комментарий (SPEC-39 §3). */
function EditLine({ line: l, mark, onClose, onSave }: { line: AcceptanceLine; mark: LineMark | undefined; onClose: () => void; onSave: (m: LineMark) => void }) {
  const num = useNumField()
  const [text, setText] = useState(() => acceptedInput(l, mark?.qty ?? null))
  const [product, setProduct] = useState(mark?.product ?? null)
  const [reason, setReason] = useState<Reason | null>(mark?.reason ?? null)
  const [comment, setComment] = useState(mark?.comment ?? '')
  const [picking, setPicking] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const qty = parseAccepted(l, text)
  const less = qty !== null && qty < l.invoice_qty
  return (
    <Modal title={l.name} wide onClose={onClose}>
      <div className="flex flex-col gap-4">
        <div className="text-base">
          По накладной: <b>{qtyWords(l, l.invoice_qty)}</b>
          {product && (
            <>
              {' '}
              → принят: <b>{product.name}</b>
            </>
          )}
        </div>
        <Field label={`Принято, ${inputUnit(l)}`}>
          <input {...num(inputKind(l), inputKind(l) === 'decimal' ? { decimals: 3 } : undefined)} value={text} onChange={(e) => setText(e.target.value)} autoFocus />
        </Field>
        <div className="flex flex-col gap-2">
          <span className="text-sm font-medium">Другой товар</span>
          {product ? (
            <div className="flex items-center gap-3">
              <span className="flex-1">{product.name}</span>
              <Button variant="secondary" onClick={() => setProduct(null)}>
                Как в накладной
              </Button>
            </div>
          ) : picking ? (
            <ProductPicker
              placeholder="Название или штрихкод товара"
              onPick={(p) => {
                if (p.unit !== l.unit) {
                  setError('Выберите товар той же единицы учёта (штуки, литры, кг)')
                  return
                }
                setError(null)
                setPicking(false)
                if (p.id !== l.product_id) setProduct({ id: p.id, name: p.name, container_ml: p.container_ml })
              }}
            />
          ) : (
            <Button variant="secondary" onClick={() => setPicking(true)}>
              Привезли другой товар
            </Button>
          )}
        </div>
        <div className="flex flex-col gap-2">
          <span className="text-sm font-medium">Причина расхождения{less ? ' (обязательна)' : ''}</span>
          <div className="flex flex-wrap gap-2">
            {REASONS.map(([k, label]) => (
              <Button key={k} variant={reason === k ? 'primary' : 'secondary'} onClick={() => setReason(reason === k ? null : k)}>
                {label}
              </Button>
            ))}
          </div>
        </div>
        <Field label="Комментарий к строке">
          <input value={comment} maxLength={500} onChange={(e) => setComment(e.target.value)} />
        </Field>
        <ErrorBox error={error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button
            onClick={() => {
              if (qty === null) return setError('Введите принятое количество')
              if (less && !reason) return setError('Принято меньше накладной — выберите причину')
              onSave({ qty, product, reason: less ? reason : null, comment: comment.trim() })
            }}
          >
            Сохранить
          </Button>
        </div>
      </div>
    </Modal>
  )
}

function ExtraModal({ initial, onClose, onSave }: { initial: Extra; onClose: () => void; onSave: (x: Extra) => void }) {
  const num = useNumField()
  const [name, setName] = useState(initial.name)
  const [qty, setQty] = useState(String(initial.qty))
  const n = /^\d{1,6}$/.test(qty.trim()) ? Number(qty.trim()) : 0
  return (
    <Modal title="Не по накладной" onClose={onClose}>
      <div className="flex flex-col gap-4">
        {initial.barcode && <div className="text-sm text-slate-600">Код: {initial.barcode}</div>}
        <Field label="Название">
          <input value={name} maxLength={120} onChange={(e) => setName(e.target.value)} autoFocus data-testid="acceptance-extra-name" />
        </Field>
        <Field label="Количество, шт">
          <input {...num('int')} value={qty} onChange={(e) => setQty(e.target.value)} data-testid="acceptance-extra-qty" />
        </Field>
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button
            disabled={n <= 0 || (!name.trim() && !initial.barcode)}
            data-testid="acceptance-extra-save"
            onClick={() => onSave({ name: name.trim(), barcode: initial.barcode, qty: n })}
          >
            Добавить
          </Button>
        </div>
      </div>
    </Modal>
  )
}

/** Шаги: кассир → PIN → подпись (у отказа — причина) → итог и отправка. */
function SignSteps({
  title,
  summary,
  refuse = false,
  onClose,
  submit,
}: {
  title: string
  summary: string
  refuse?: boolean
  onClose: () => void
  submit: (employeeId: string, pin: string, signature: string, reason: string) => Promise<void>
}) {
  const num = useNumField()
  const act = useAction()
  const employees = useLoad(() => get<Employee[]>('/employees'), [])
  // Кассиры с PIN: активные сотрудники филиала с отметкой «кассир» и заданным PIN.
  const cashiers = (employees.data ?? []).filter((e) => e.active && e.is_cashier && e.has_pin)
  const [who, setWho] = useState<Employee | null>(null)
  const [pin, setPin] = useState('')
  const [pinDone, setPinDone] = useState(false)
  const [signature, setSignature] = useState<string | null>(null)
  const [reason, setReason] = useState('')
  const stage = !who ? 'who' : !pinDone ? 'pin' : refuse ? (reason.trim() && signature === '' ? 'final' : 'reason') : signature ? 'final' : 'sign'

  return (
    <Modal title={title} wide onClose={onClose}>
      <div className="flex flex-col gap-4" data-testid="acceptance-steps" data-stage={stage}>
        {stage === 'who' && (
          <>
            <div className="text-lg font-semibold">Кто принимает?</div>
            <ErrorBox error={employees.error} />
            {employees.data && cashiers.length === 0 && <Notice>Нет кассиров с PIN — PIN задаёт владелец в карточке сотрудника</Notice>}
            <div className="grid grid-cols-2 gap-3">
              {cashiers.map((e) => (
                <Button key={e.id} variant="secondary" className="min-h-[64px] text-lg" data-testid="acceptance-cashier" onClick={() => setWho(e)}>
                  {e.full_name}
                </Button>
              ))}
            </div>
          </>
        )}
        {stage === 'pin' && who && (
          <>
            <div className="text-lg">
              {who.full_name}: введите PIN
            </div>
            <Field label="PIN кассира">
              <input
                {...num('digits')}
                type="password"
                autoFocus
                data-testid="acceptance-pin"
                value={pin}
                onChange={(e) => setPin(e.target.value.replace(/\D/g, '').slice(0, 4))}
              />
            </Field>
            <div className="flex justify-between gap-2">
              <Button variant="secondary" onClick={() => setWho(null)}>
                ← Другой кассир
              </Button>
              <Button disabled={!/^\d{4}$/.test(pin)} data-testid="acceptance-pin-next" onClick={() => setPinDone(true)}>
                Дальше
              </Button>
            </div>
          </>
        )}
        {stage === 'reason' && (
          <>
            <Field label="Причина отказа">
              <input value={reason} maxLength={500} autoFocus data-testid="acceptance-refuse-reason" onChange={(e) => setReason(e.target.value)} />
            </Field>
            <div className="flex justify-end">
              <Button disabled={!reason.trim()} onClick={() => setSignature('')}>
                Дальше
              </Button>
            </div>
          </>
        )}
        {stage === 'sign' && (
          <>
            <div className="text-lg">{who?.full_name}, распишитесь</div>
            <SignaturePad onDone={setSignature} />
          </>
        )}
        {stage === 'final' && (
          <>
            <div className="text-xl font-semibold" data-testid="acceptance-summary">
              {summary}
            </div>
            <div className="text-base text-slate-600">Кассир: {who?.full_name}</div>
            <ErrorBox error={act.error} />
            <div className="flex justify-between gap-2">
              <Button
                variant="secondary"
                disabled={act.busy}
                onClick={() => {
                  // Неверный PIN или отказ сервера — ввести заново; отметки остаются.
                  setPin('')
                  setPinDone(false)
                  setSignature(null)
                  act.setError(null)
                }}
              >
                ← Заново
              </Button>
              <Button
                className="px-8 text-xl"
                variant={refuse ? 'danger' : 'primary'}
                data-testid="acceptance-submit"
                disabled={act.busy}
                onClick={() => void act.run(() => submit(who?.id ?? '', pin, signature ?? '', reason.trim()))}
              >
                {refuse ? 'Отказаться от приёмки' : 'Принять товар'}
              </Button>
            </div>
          </>
        )}
      </div>
    </Modal>
  )
}
