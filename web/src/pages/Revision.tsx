// Ревизия склада: пересчёт сканером по категориям, проведение владельцем (SPEC-15). Один экран с
// «Работой с товарами» (SPEC-20): вкладки «Ревизия» и «Сканер товаров»; в ревизию вписывают и
// товар, которого нет в программе — карточка заводится с найденным количеством (SPEC-29 §10).
import { useCallback, useEffect, useRef, useState } from 'react'
import { useSearchParams } from 'react-router-dom'
import { Pager, usePager } from '../components/Pager'
import { ExportButton } from '../components/ExportButton'
import { colsIf, listSheet, totalBy, unitName, xl, type ExportCol } from '../lib/exportTable'
import { ProductFormModal } from '../components/ProductFormModal'
import { RevisionNewProduct } from '../components/RevisionNewProduct'
import { RevisionSearchHits } from '../components/RevisionSearchHits'
import { CameraScanButton } from '../components/CameraScan'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, Modal, Notice, PageHeader, Table, toast } from '../components/ui'
import { ApiError, del, get, newOpId, post, put, watchServerReachable } from '../lib/api'
import { useUser } from '../lib/auth'
import { OilCountInput, type OilCount } from '../components/OilCountInput'
import { formatDateTime, formatSom, litersInput, parseLiters, plural } from '../lib/format'
import { countText, kgInput, oilFields, parseKg, parseOil, viscosityFromName, viscosityKey } from '../lib/units'
import { errorText, useAction, useLoad } from '../lib/hooks'
import { useCan } from '../lib/permissions'
import type { Category, OldCatalogHit, Product } from '../lib/types'
import { sameCode } from '../lib/umagCodes'
import { retrySame } from '../lib/retrySame'
import {
  addIsEmpty,
  appendCount,
  countLogText,
  countSummary,
  mineOf,
  defaultCountMode,
  expectedBody,
  expectedQuery,
  freshRevision,
  InputOps,
  inputKey,
  OwnWrites,
  patchIfNewer,
  type CountEntry,
  type CountMode,
  type Seen,
} from '../lib/revisionCount'
import {
  browserLocks,
  cleanQueues,
  closedText,
  newScan,
  otherQueues,
  queueAdd,
  queueResolve,
  queueToSend,
  queueWithout,
  queueWriter,
  readQueue,
  scansText,
  sendOutcome,
  unsentByLine,
  unsentCodes,
  unsentCount,
  unsentFirstText,
  unsentText,
  withQueueLock,
  type QueuedScan,
  type SendOutcome,
} from '../lib/revisionQueue'
import ProductIntake from './ProductIntake'

interface Head {
  id: string
  number: number
  category_id: string | null
  category_name: string | null
  status: 'draft' | 'posted' | 'cancelled'
  comment: string
  user_name: string
  created_at: string
  posted_at: string | null
  counted: number
}

interface Line {
  product_id: string
  name: string
  article: string
  barcodes: string[]
  unit: 'piece' | 'ml' | 'g'
  container_ml: number | null
  expected_qty: number
  counted_qty: number | null
  value_delta_tyiyn?: number
  /** Вязкость масла из карточки (SPEC-36 §13). */
  viscosity?: string | null
  /** Кто записал строку последним: считают несколько человек с разных устройств (задача 003, Н14). */
  counted_by?: string | null
  /** Кто сколько посчитал: «Айбек +2, Нурлан +1, Азиз = 5». */
  counted_log?: CountEntry[]
  /** Номер последней записи строки на сервере: показывается только ответ новее показанного. */
  version?: number
}

/** Ответ записи строки: общий итог (убрали строку — нет) и номер записи. */
interface Saved {
  counted_qty: number | null
  version?: number
}

interface RevisionOut {
  head: Head
  lines: Line[]
  /** Норма расхождения по маслу, сотые доли процента (ADR-052). */
  oil_norm_bp: number
  /** Номер последней записи пересчёта ревизии. */
  version?: number
}

/** Сотые доли процента для показа: 50 → «0,5 %». */
const percentText = (bp: number): string => `${String(bp / 100).replace('.', ',')} %`

/** «0,5» → 50; больше двух знаков после запятой или не число — null. */
function parsePercentBp(s: string): number | null {
  const m = /^(\d{1,2})(?:[.,](\d{1,2}))?$/.exec(s.trim())
  if (!m) return null
  return Number(m[1]) * 100 + Number((m[2] ?? '').padEnd(2, '0'))
}

/** Масло: расхождение больше нормы — как на сервере, целыми. */
const overNorm = (l: Line, normBp: number): boolean =>
  l.unit === 'ml' && l.counted_qty !== null && Math.abs(l.counted_qty - l.expected_qty) * 10000 > Math.max(l.expected_qty, 0) * normBp

const STATUS: Record<Head['status'], [string, 'amber' | 'green' | 'slate']> = {
  draft: ['идёт пересчёт', 'amber'],
  posted: ['проведена', 'green'],
  cancelled: ['отменена', 'slate'],
}

/** Масло в таре: вводится «кан. + л» (SPEC-36 §13). */
const oilTare = (l: Pick<Line, 'unit' | 'container_ml'>): number | null => (l.unit === 'ml' && l.container_ml ? l.container_ml : null)

/** Поле пересчёта: штуки целым числом, масло без тары литрами, на вес — кг; все дробные — до 3 знаков. */
function parseCount(l: Line, text: string): number | null {
  const t = text.trim()
  if (!t) return null
  if (l.unit === 'ml') return parseLiters(t)
  if (l.unit === 'g') return parseKg(t)
  return /^\d+$/.test(t) ? Number(t) : null
}

/** Посчитанное в поле: литры или кг — с запятой; граммы в поле не показываются (ошибка ×1000). */
const countInput = (l: Line, q: number): string => (l.unit === 'ml' ? litersInput(q) : l.unit === 'g' ? kgInput(q) : String(q))

/** Строки ревизии перечитываются раз в 5 с и при возврате на вкладку: каждый видит общий итог. */
const REFRESH_MS = 5_000

/** Вязкость строки: из карточки; у масла без неё — из названия, с пометкой. */
/** Вид ревизии у владельца на этом устройстве: `all` — «Все вместе», иначе «Моё» (задача 007). */
const VIEW_KEY = 'avtodom.revision.view'

function viscosityOf(l: Pick<Line, 'unit' | 'name' | 'viscosity'>): { v: string; fromName: boolean } | null {
  if (l.viscosity) return { v: l.viscosity, fromName: false }
  if (l.unit !== 'ml') return null
  const v = viscosityFromName(l.name)
  return v ? { v, fromName: true } : null
}

function Draft({
  rev,
  onChanged,
  onFresh,
  reload,
}: {
  rev: RevisionOut
  onChanged: () => void
  /** Свой ответ сервера (итог строки после своей записи) — показать сразу. */
  onFresh: (update: (r: RevisionOut) => RevisionOut) => void
  /** Перечитать ревизию: общий итог с других устройств; устаревший ответ не показывается. */
  reload: () => Promise<void>
}) {
  const me = useUser()
  const owner = me.role === 'owner'
  const [scan, setScan] = useState('')
  const [filter, setFilter] = useState('')
  const [edits, setEdits] = useState<Record<string, string>>({})
  /** Масло в таре: поля «кан.» и «+ л» строки, пока их правят. */
  const [oilEdits, setOilEdits] = useState<Record<string, OilCount>>({})
  /** Режим поля строки: выбранный человеком или застывший с начала ввода (задача 003, Н14). */
  const [modes, setModes] = useState<Record<string, CountMode>>({})
  /** Товары, которых нет в загруженном списке (их только что завёл другой): строка — до перечитывания. */
  const [extra, setExtra] = useState<Line[]>([])
  /** «= итог» не записан: пока вводили, итог строки изменился — переспросить (`now` null — строки нет). */
  const [ask, setAsk] = useState<{ line: Line; qty: number | null; now: Seen } | null>(null)
  /** Отбор по вязкости (ключ `viscosityKey`), пусто — все. */
  const [visc, setVisc] = useState('')
  /** Окно проведения; `warn` — «Ещё считают: …» с сервера, проведение — после подтверждения. */
  const [posting, setPosting] = useState<{ comment: string; opId: string; warn?: string } | null>(null)
  /** «Товар не из программы»: окно новой карточки, с кодом, если его отсканировали, или из строки
   *  Умага, выбранной в поиске по названию (задача 005, Н7). */
  const [adding, setAdding] = useState<{ barcode?: string; hit?: OldCatalogHit } | null>(null)
  /** Заводить из строки Умага в поиске — право «Товары и штрихкоды» (SPEC-35 §4). */
  const mayUmag = useCan()('products')
  /** Заведён товар из окна «Товар не из программы»: поиск вне списка перечитывается. */
  const [addedCount, setAddedCount] = useState(0)
  /** Карточка товара из строки (задача 002, П5): с правом «Товары и штрихкоды» — правка, иначе просмотр. */
  const [card, setCard] = useState<Product | null>(null)
  const act = useAction()
  const draft = rev.head.status === 'draft'
  /** Владелец смотрит «Все вместе» — итог строк и кто сколько; иначе каждый видит только свой
   *  пересчёт (задача 007). */
  const [allView, setAllViewState] = useState(() => localStorage.getItem(VIEW_KEY) === 'all')
  /** Выбор владельца запоминается на устройстве. */
  const setAllView = (v: boolean) => {
    localStorage.setItem(VIEW_KEY, v ? 'all' : 'own')
    setAllViewState(v)
  }
  const ownView = draft && !(owner && allView)
  /** Сотрудник во время пересчёта не видит учётного остатка и расхождений — слепой пересчёт. */
  const blind = draft && !owner
  // Строка, появившаяся до перечитывания, — новее загруженной, пока перечитывание её не догонит.
  const rawLines = [
    ...rev.lines.map((x) => {
      const e = extra.find((y) => y.product_id === x.product_id)
      return e && (e.version ?? -1) > (x.version ?? -1) ? e : x
    }),
    ...extra.filter((e) => !rev.lines.some((x) => x.product_id === e.product_id)),
  ]
  // Свой вид: в строке — своё число (сумма своих прибавок), а не общий итог (задача 007).
  const allLines = ownView ? rawLines.map((x) => ({ ...x, counted_qty: mineOf(x.counted_log ?? [], me.id), counted_by: null })) : rawLines

  /** Товар этой ревизии: весь склад или её категория. */
  const fitsRevision = (p: Pick<Product, 'category_id'>) => !rev.head.category_id || p.category_id === rev.head.category_id
  /** Строка для товара, которого ещё нет в загруженном списке. */
  const lineOf = (p: Product): Line => ({
    product_id: p.id,
    name: p.name,
    article: p.article,
    barcodes: p.barcodes,
    unit: p.unit,
    container_ml: p.container_ml,
    expected_qty: p.stock_qty ?? 0,
    counted_qty: null,
  })

  /** Свой вид: строки, которые человек открыл сам (поиск «Посчитать», товар на вес после скана), —
   *  видны, пока он их не посчитал (задача 007). */
  const [opened, setOpened] = useState<ReadonlySet<string>>(() => new Set())
  const openLine = (id: string) => setOpened((o) => (o.has(id) ? o : new Set(o).add(id)))
  const focusLine = (l: Line) => {
    openLine(l.product_id)
    setFilter(l.name)
    setVisc('')
    setTimeout(() => document.querySelector<HTMLInputElement>(`[data-count="${l.product_id}"]`)?.focus(), 50)
  }

  /** Товар уже есть в программе: показать его строку и поставить курсор в поле пересчёта. Строки
   *  ещё нет на экране, а товар этой ревизии — строка появляется сразу. */
  const goTo = (p: Product) => {
    setAdding(null)
    const l = allLines.find((x) => x.product_id === p.id)
    if (l) return focusLine(l)
    if (!fitsRevision(p)) {
      act.setError(`«${p.name}» не входит в эту ревизию${rev.head.category_name ? ` (категория «${rev.head.category_name}»)` : ''}`)
      return
    }
    const line = lineOf(p)
    setExtra((e) => [...e.filter((x) => x.product_id !== p.id), line])
    void reload()
    focusLine(line)
  }

  // Правки — и в ref: Enter и уход из поля читают набранное сейчас, а не из прошлой отрисовки.
  const editsNow = useRef(edits)
  const oilEditsNow = useRef(oilEdits)
  const modesNow = useRef(modes)
  /** Свои записи строк: «= итог» и очистка сверяются на сервере с тем, что человек видел, когда
   *  начал ввод, плюс его собственные записи после этого (409 — пока вводили, другие прибавили;
   *  чужие сканы не затираются). */
  const [own] = useState(() => new OwnWrites())
  /** op_id ввода в поле строки: тот же не записанный ввод — тот же op_id (ADR-062). */
  const [ops] = useState(() => new InputOps(newOpId))
  /** op_id записей, которые ещё идут: тот же ввод второй раз не отправляется. */
  const inflight = useRef(new Set<string>())
  /** Режим, допустимый в этом виде: в своём — «+ добавить» и «= моё», в общем — «+ добавить» и «= итог». */
  const fitMode = (m: CountMode | undefined): CountMode => (!m || (ownView ? m === 'set' : m === 'mine') ? defaultCountMode() : m)
  const modeOf = (l: Line): CountMode => fitMode(modesNow.current[l.product_id])
  const putMode = (id: string, m: CountMode) => {
    modesNow.current = { ...modesNow.current, [id]: m }
    setModes(modesNow.current)
  }
  /** Ввод начался: режим и итог на экране застывают — чужая прибавка не меняет смысл числа в поле. */
  const begin = (l: Line) => {
    const id = l.product_id
    if (editsNow.current[id] !== undefined || oilEditsNow.current[id] !== undefined) return
    putMode(id, modeOf(l))
    own.see(id, l.counted_qty)
  }
  /** Человек выбрал режим сам: набранное остаётся и читается по-новому. */
  const switchMode = (l: Line, m: CountMode) => {
    putMode(l.product_id, m)
    own.see(l.product_id, l.counted_qty)
  }
  const putEdit = (l: Line, text: string) => {
    begin(l)
    editsNow.current = { ...editsNow.current, [l.product_id]: text }
    setEdits(editsNow.current)
  }
  const putOilEdit = (l: Line, val: OilCount) => {
    begin(l)
    oilEditsNow.current = { ...oilEditsNow.current, [l.product_id]: val }
    setOilEdits(oilEditsNow.current)
  }
  /** Убрать правку поля, если её не поменяли с `was` (не передан — в любом случае). */
  const dropEdit = (id: string, was?: { text?: string; oil?: OilCount }) => {
    const drop = <T,>(cur: Record<string, T>, sent: T | undefined): Record<string, T> | null => {
      if (cur[id] === undefined || (was && cur[id] !== sent)) return null
      const next = { ...cur }
      delete next[id]
      return next
    }
    const e = drop(editsNow.current, was?.text)
    if (e) {
      editsNow.current = e
      setEdits(e)
    }
    const o = drop(oilEditsNow.current, was?.oil)
    if (o) {
      oilEditsNow.current = o
      setOilEdits(o)
    }
  }
  /** Строка по ответу сервера — и в загруженной ревизии, и среди только что появившихся; ответ
   *  старее показанной строки (`version`) не показывается. */
  const setLine = (id: string, version: number | undefined, patch: (x: Line) => Line) => {
    const fresh = (x: Line) => (x.product_id === id ? patchIfNewer(x, version, patch) : x)
    onFresh((r) => ({ ...r, lines: r.lines.map(fresh) }))
    setExtra((e) => (e.some((x) => x.product_id === id) ? e.map(fresh) : e))
  }
  // Записи одной строки — по очереди: вторая правка уходит после ответа на первую, иначе на
  // загруженном сервере первая могла дойти позже и затереть вторую (12,5 вместо 12,6).
  const writes = useRef(new Map<string, Promise<Saved>>())

  /** Неотправленные сканы этой ревизии и этого пользователя на устройстве: в localStorage,
   *  переживают перезагрузку, общие для вкладок; уходят при связи с теми же op_id (задача 003, Н14). */
  const revId = rev.head.id
  const user = { id: me.id, name: me.full_name }
  const [queue, setQueue] = useState<QueuedScan[]>(() => readQueue(localStorage, revId, me.id) ?? [])
  /** Запись — слияние со свежей очередью из хранилища под блокировкой вкладок: скан другой вкладки
   *  не затирается; на экране изменение сразу. */
  const [writer] = useState(() => queueWriter(localStorage, revId, user, browserLocks(), setQueue))
  /** Свои сканы, которые сейчас отправляются впервые: ещё не «не отправлено». */
  const [fresh] = useState(() => new Set<string>())
  const putQueue = (f: (q: QueuedScan[]) => QueuedScan[]) => void writer.put(f)
  /** Неотправленные сканы этой ревизии под другими пользователями этого устройства — не отправляются. */
  const [others, setOthers] = useState(() => otherQueues(localStorage, revId, me.id))
  // Другая вкладка изменила очередь (свой скан, отправила) — отметки ⏳ здесь те же.
  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key !== null && !e.key.startsWith('avtodom.revision.unsent.')) return
      writer.reload()
      setOthers(otherQueues(localStorage, revId, me.id))
    }
    window.addEventListener('storage', onStorage)
    return () => window.removeEventListener('storage', onStorage)
  }, [writer, revId, me.id])
  /** Сканы из очереди, которые сервер окончательно не принял: на экране, пока не нажмут «Понятно». */
  const [closedLost, setClosedLost] = useState(0)
  const [lostNotes, setLostNotes] = useState<string[]>([])
  const [lostCodes, setLostCodes] = useState<{ code: string; why: 'not_found' | 'not_fit' | 'need_qty' }[]>([])
  /** Сессия кончилась (401): сканы ждут входа. */
  const [needLogin, setNeedLogin] = useState(false)
  const note = (t: string) => setLostNotes((n) => (n.includes(t) ? n : [...n, t]))

  /**
   * Запись строки — по очереди с прошлыми записями этой строки; `null` — этот ввод уже
   * отправляется. `shown` — итог на экране, если ввод не начинали (сверка «= итог»). Ответа нет —
   * тот же запрос повторяется сам (`tries` раз) с тем же op_id. `gate` — проверка перед отправкой,
   * когда прошлые записи строки ответили: бросила — запись не уходит.
   */
  const send = (
    id: string,
    shown: Seen,
    qty: number | null,
    mode: CountMode,
    opId: string,
    tries = 2,
    gate?: () => void,
  ): Promise<Saved> | null => {
    if (inflight.current.has(opId)) return null
    inflight.current.add(opId)
    const prev = writes.current.get(id)
    const seq = own.start(id, qty === null ? 'clear' : mode, qty ?? 0)
    const path = `/revisions/${revId}/lines`
    const write = (prev ?? Promise.resolve(null))
      .catch(() => null)
      .then(() => {
        gate?.()
        // С чем сверять — когда своя прошлая запись строки уже ответила; ввод не начинали — что на экране.
        const base = own.baseOr(id, seq, shown)
        return retrySame(
          () =>
            qty === null
              ? del<Saved>(`${path}/${id}?${expectedQuery(base)}&op_id=${opId}`)
              : put<Saved>(path, { product_id: id, counted_qty: qty, mode, op_id: opId, ...(mode === 'set' ? expectedBody(base) : {}) }),
          tries,
        )
      })
      .then(
        (r) => {
          own.settle(id, seq, true)
          return r
        },
        (e: unknown) => {
          own.settle(id, seq, false)
          throw e
        },
      )
    writes.current.set(id, write)
    const done = () => {
      inflight.current.delete(opId)
      if (writes.current.get(id) === write) writes.current.delete(id)
    }
    write.then(done, done)
    return write
  }

  /** Своя запись записана: итог строки — из ответа, в журнале строки — своя запись. */
  const applySaved = (id: string, qty: number | null, mode: CountMode, res: Saved) => {
    const total = res.counted_qty
    // «= моё» сервер записал прибавкой разницы со своим числом строки — так же и в журнал на экране.
    const entryOf = (x: Line): CountEntry =>
      qty === null
        ? { by: me.full_name, user_id: me.id, mode: 'clear', qty: 0 }
        : mode === 'mine'
          ? { by: me.full_name, user_id: me.id, mode: 'add', qty: qty - (mineOf(x.counted_log ?? [], me.id) ?? 0) }
          : { by: me.full_name, user_id: me.id, mode, qty }
    setLine(id, res.version, (x) => ({
      ...x,
      counted_qty: total,
      counted_by: total === null ? null : me.full_name,
      counted_log: appendCount(x.counted_log ?? [], entryOf(x)),
    }))
    // Строки не было в загруженном списке — перечитываем, чтобы она стала настоящей.
    if (!rev.lines.some((x) => x.product_id === id)) void reload()
  }

  /**
   * `add` — прибавить к общему итогу строки («+ добавить»): 4 человека нашли по одному — 4, а не 1.
   * `set` — «= итог», сверка с тем, что человек видел, когда начал ввод, плюс его собственные
   * записи после этого (`own`): пока вводили, другие прибавили — сервер отвечает 409, строка
   * перечитывается и человека переспрашивают. `qty === null` в `set` — убрать строку.
   * `opId` — op_id ввода: повтор того же ввода (автоматический после обрыва или ещё одно нажатие
   * с тем же числом в поле) не прибавит дважды. На экране — итог из ответа сервера (задача 003, Н14).
   */
  const save = (l: Line, qty: number | null, mode: CountMode, opId: string) => {
    // Записанный ввод убирается из правок, только если его не успели поменять, пока шёл запрос:
    // иначе новое число пропало бы, не записавшись.
    const id = l.product_id
    const was = { text: editsNow.current[id], oil: oilEditsNow.current[id] }
    // «= итог» и очистка — после неотправленных сканов этой строки, а не до них: иначе прибавка
    // из очереди ушла бы после итога и прибавилась к нему. Сканы уходят первыми в той же цепочке
    // записей строки и, записавшись, входят в сверку как свои (человек их видел с отметкой ⏳);
    // не ушли — итог не записывается, число остаётся в поле.
    const before = mode !== 'add' ? writer.current.filter((x) => x.productId === id).map((x) => x.opId) : []
    for (const s of writer.current) if (before.includes(s.opId) && !inflight.current.has(s.opId)) void sendScan(s, 0)
    const gate =
      before.length === 0
        ? undefined
        : () => {
            const left = writer.current.filter((x) => before.includes(x.opId)).length
            if (left > 0) throw new Error(unsentFirstText(left))
          }
    const write = send(id, l.counted_qty, qty, mode, opId, 2, gate)
    if (!write) return
    void act.run(async () => {
      let res: Saved | null = null
      try {
        res = await write
      } catch (e) {
        // Ревизию провели или отменили, пока считали: сообщение — с сервера, экран — заново.
        if (e instanceof ApiError && e.code === 'revision_closed') void reload()
        if (!(e instanceof ApiError && e.code === 'count_changed')) throw e
      }
      if (!res) {
        // Пока вводили, итог изменился: перечитываем строку и переспрашиваем.
        const now = await get<Saved>(`/revisions/${revId}/lines/${id}`)
        setLine(id, now.version, (x) => ({ ...x, counted_qty: now.counted_qty }))
        setAsk({ line: l, qty, now: now.counted_qty })
        return
      }
      ops.done(id, opId)
      applySaved(id, qty, mode, res)
      dropEdit(id, was)
      void flush()
    })
  }

  /** Сервер окончательно не принял скан из очереди: ревизию провели — в общее число «не учтены»,
   *  иначе — причина; на экране, пока не нажмут «Понятно» (следующий скан её не стирает). */
  const refusedScan = (e: unknown) => {
    if (e instanceof ApiError && e.code === 'revision_closed') {
      setClosedLost((n) => n + 1)
      void reload()
    } else note(`Скан не учтён: ${errorText(e)}`)
  }
  /** Ждать связи или входа: отметки ⏳ остаются, 401 — «Войдите снова». */
  const waitScan = (outcome: SendOutcome) => {
    if (outcome === 'login') setNeedLogin(true)
  }

  /**
   * Скан из очереди на сервер (тот же op_id). Записан — уходит из очереди; ответа нет, 5xx, 429,
   * 401 — остаётся с отметкой «не отправлено»; сервер окончательно отказал по существу (ревизию
   * провели, неверные данные) — уходит, причина — на экране. Убирается только свой op_id: сканы
   * других вкладок остаются.
   */
  const sendScan = async (s: QueuedScan, tries: number): Promise<SendOutcome | 'busy'> => {
    if (s.productId === null) return resolveScan(s)
    const productId = s.productId
    const write = send(productId, null, s.qty, 'add', s.opId, tries)
    if (!write) return 'busy'
    let outcome: SendOutcome = 'ok'
    let res: Saved | null = null
    try {
      res = await write
    } catch (e) {
      outcome = sendOutcome(e)
      if (outcome === 'refused') refusedScan(e)
      else waitScan(outcome)
    }
    fresh.delete(s.opId)
    if (outcome === 'ok' || outcome === 'refused') putQueue((q) => queueWithout(q, [s.opId]))
    else setQueue([...writer.current])
    if (outcome === 'ok') setNeedLogin(false)
    if (res) applySaved(productId, s.qty, 'add', res)
    return outcome
  }

  /** Строка товара на экране: её нет в загруженном списке — появляется сразу, до перечитывания. */
  const showLine = (p: Product) => setExtra((e) => (e.some((x) => x.product_id === p.id) ? e : [...e, lineOf(p)]))

  /**
   * Скан кода, которого не было на экране, отсканированный без связи: товар — по коду на сервере.
   * Нашёлся и подходит ревизии — тот же op_id уходит прибавкой к его строке; не нашёлся, не этой
   * ревизии или считается не сканом (на вес, масло без тары) — скан уходит из очереди, коды — на
   * экране списком.
   */
  const resolveScan = async (s: QueuedScan): Promise<SendOutcome | 'busy'> => {
    const code = s.barcode ?? ''
    if (inflight.current.has(s.opId)) return 'busy'
    inflight.current.add(s.opId)
    let p: Product
    try {
      p = await get<Product>(`/products/by-barcode/${encodeURIComponent(code)}`)
    } catch (e) {
      const outcome = sendOutcome(e)
      if (outcome === 'refused') {
        if (e instanceof ApiError && e.status === 404) setLostCodes((c) => [...c, { code, why: 'not_found' }])
        else note(`Скан ${code} не учтён: ${errorText(e)}`)
        putQueue((q) => queueWithout(q, [s.opId]))
      } else waitScan(outcome)
      return outcome
    } finally {
      inflight.current.delete(s.opId)
    }
    const qty = p.unit === 'piece' ? 1 : oilTare(p)
    const why = !fitsRevision(p) ? 'not_fit' : qty === null ? 'need_qty' : null
    if (why || qty === null) {
      if (why === 'need_qty') showLine(p)
      setLostCodes((c) => [...c, { code, why: why ?? 'need_qty' }])
      putQueue((q) => queueWithout(q, [s.opId]))
      return 'refused'
    }
    showLine(p)
    putQueue((q) => queueResolve(q, s.opId, p.id, qty))
    return sendScan({ ...s, productId: p.id, barcode: null, qty }, 0)
  }

  /** Неотправленные сканы — на сервер по порядку; связи по-прежнему нет — до следующего раза. */
  const flushing = useRef(false)
  const flush = async () => {
    if (flushing.current) return
    flushing.current = true
    try {
      for (const s of queueToSend(writer.current, inflight.current)) {
        // Пока шли прошлые, скан могли отправить «= итог» строки или другая вкладка.
        if (!writer.current.some((x) => x.opId === s.opId)) continue
        const r = await sendScan(s, 0)
        if (r === 'wait' || r === 'login') break
      }
    } finally {
      flushing.current = false
    }
  }
  const flushNow = useRef(flush)
  useEffect(() => {
    flushNow.current = flush
  })

  /** Скан: штука или канистра — прибавка в очередь (сразу в localStorage) и на сервер. Ответа нет
   *  и после повторов — скан остаётся в очереди с отметкой у строки, а не пропадает молча. */
  const scanAdd = (l: Line, qty: number) => {
    const s = newScan(newOpId(), { productId: l.product_id, qty })
    fresh.add(s.opId)
    putQueue((q) => queueAdd(q, s))
    void sendScan(s, 2).then((r) => {
      // Связь есть — заодно уходят сканы, не отправленные раньше.
      if (r === 'ok') void flush()
    })
  }

  // Общий итог с других устройств: перечитываем, пока ничего не пишется; поле, которое сейчас
  // правят, не затирается — правка живёт в `edits`, пока её не запишут. Строка, показанная новее
  // ответа, не заменяется (`reload` сверяет версии строк). Неотправленные сканы уходят раз в 5 с,
  // при возврате связи и при следующем ответе сервера.
  useEffect(() => {
    if (!draft) return
    const refresh = () => {
      if (writer.current.some((x) => !fresh.has(x.opId))) void flushNow.current()
      if (document.visibilityState !== 'visible' || writes.current.size > 0) return
      void reload()
    }
    const online = () => void flushNow.current()
    const t = setInterval(refresh, REFRESH_MS)
    document.addEventListener('visibilitychange', refresh)
    window.addEventListener('online', online)
    const stop = watchServerReachable((up) => up && online())
    return () => {
      clearInterval(t)
      document.removeEventListener('visibilitychange', refresh)
      window.removeEventListener('online', online)
      stop()
    }
  }, [revId, draft, reload, fresh, writer])

  /** Поле ушло из фокуса: «+ добавить» прибавляет набранное, «= итог» записывает итог строки.
   *  Тот же не записанный ввод (после обрыва в поле осталось то же число) — с тем же op_id. */
  const commit = (l: Line, qty: number | null) => {
    const id = l.product_id
    const mode = modeOf(l)
    if (mode === 'add' && addIsEmpty(qty, l.counted_qty)) return dropEdit(id)
    // «= моё»: пустое поле или то же своё число — менять нечего.
    if (mode === 'mine' && (qty === null || qty === l.counted_qty)) return dropEdit(id)
    // Пустую строку очистили, своих записей в пути и неотправленных сканов нет — убирать нечего.
    if (
      mode === 'set' &&
      qty === null &&
      own.base(id, Number.MAX_SAFE_INTEGER) === null &&
      !writes.current.has(id) &&
      !writer.current.some((x) => x.productId === id)
    )
      return dropEdit(id)
    save(l, qty, mode, ops.take(id, inputKey(mode, qty)))
  }

  /** Поля масла ушли из фокуса: «кан. + л» → мл; оба пустые — в «= итог» строка не посчитана. */
  const saveOil = (l: Line, tare: number) => {
    const e = oilEditsNow.current[l.product_id]
    if (!e) return
    const ml = parseOil(e.cans, e.liters, tare)
    if (ml === null) {
      act.setError(`Неверное количество: ${l.name}`)
      return
    }
    commit(l, ml === 'empty' ? null : ml)
  }

  // Поиск — по названию, артикулу и вязкости; отбор «Вязкость» — по строкам ревизии (SPEC-36 §13).
  const q = filter.trim().toLowerCase()
  const qKey = viscosityKey(filter)
  const viscs = new Map<string, string>()
  for (const l of allLines) {
    const v = viscosityOf(l)
    if (v && !viscs.has(viscosityKey(v.v))) viscs.set(viscosityKey(v.v), v.v)
  }
  const viscOptions = [...viscs.entries()].sort((a, b) => a[1].localeCompare(b[1], 'ru', { numeric: true }))
  // Свой вид — только своё: посчитанное этим входом, открытое им самим и его неотправленные сканы;
  // чужие строки и непосчитанный склад не показываются, их находят сканером или поиском (задача 007).
  const pendingIds = new Set(unsentByLine(queue, fresh).keys())
  const shownLines = ownView
    ? allLines.filter((l) => l.counted_qty !== null || opened.has(l.product_id) || edits[l.product_id] !== undefined || pendingIds.has(l.product_id))
    : allLines
  const lines = shownLines.filter((l) => {
    const v = viscosityOf(l)
    if (visc && (!v || viscosityKey(v.v) !== visc)) return false
    if (!q) return true
    return l.name.toLowerCase().includes(q) || l.article.toLowerCase().includes(q) || (v !== null && qKey.length >= 3 && viscosityKey(v.v).includes(qKey))
  })
  // Длинный склад — по страницам; поиск в списке возвращает на первую.
  const pager = usePager(lines, 'revision.lines', `${q}|${visc}`)

  /** Товар строки посчитан сканом: штучный +1, масло в таре +1 канистра — курсор остаётся в сканере;
   *  на вес и масло без тары — фокус на поле количества (на его странице) в режиме «+ добавить»:
   *  тот же товар в другом месте может считать другой человек. */
  const countLine = (l: Line) => {
    act.setError(null)
    const tare = oilTare(l)
    // Прибавка, а не «своё число + 1»: другие устройства считают ту же строку (задача 003, Н14).
    // Каждый скан — новый ввод со своим op_id; его повтор (автоматический после обрыва и из
    // очереди неотправленных) — тот же.
    if (l.unit === 'piece') scanAdd(l, 1)
    else if (tare) scanAdd(l, tare)
    else {
      if (editsNow.current[l.product_id] === undefined) switchMode(l, 'add')
      openLine(l.product_id)
      const at = lines.findIndex((x) => x.product_id === l.product_id)
      if (at >= 0) pager.setPage(Math.floor(at / pager.size))
      // Поле появится после перехода на страницу.
      setTimeout(() => document.querySelector<HTMLInputElement>(`[data-count="${l.product_id}"]`)?.focus(), 50)
    }
  }

  // Скан: код строки ревизии — и с потерянными Excel нулями впереди (код Умага, SPEC-35 §1); нет —
  // товар по коду на сервере: этой ревизии — его строка (появится, если её ещё нет на экране), вне
  // её — сообщение; нигде — новая карточка.
  const onScan = (camera?: string) => {
    const code = (camera ?? scan).trim()
    if (!code) return
    const l = allLines.find((x) => x.barcodes.includes(code)) ?? allLines.find((x) => x.barcodes.some((b) => sameCode(b, code)))
    setScan('')
    if (l) {
      countLine(l)
      return
    }
    void retrySame(() => get<Product>(`/products/by-barcode/${encodeURIComponent(code)}`))
      .then((p) => {
        const mine = allLines.find((x) => x.product_id === p.id)
        if (mine) return countLine(mine)
        if (!fitsRevision(p)) return goTo(p)
        // Товар этой ревизии, а строки на экране нет (его только что завёл другой или остатка не
        // было): строка появляется сразу, список перечитывается; штуку и канистру считаем сразу,
        // на вес и масло без тары — поле открыто.
        const line = lineOf(p)
        setExtra((e) => [...e.filter((x) => x.product_id !== p.id), line])
        setFilter(p.name)
        setVisc('')
        void reload()
        countLine(line)
      })
      .catch((e) => {
        if (e instanceof ApiError && e.status === 404) {
          // Нет в ревизии: товара нет и в программе — предлагаем завести с найденным количеством.
          act.setError(null)
          setAdding({ barcode: code })
          return
        }
        const outcome = sendOutcome(e)
        if (outcome === 'refused') return act.setError(errorText(e))
        // Связи нет: скан не пропадает — в очередь по коду со своим op_id; товар уточнится при связи.
        act.setError(null)
        waitScan(outcome)
        putQueue((q) => queueAdd(q, newScan(newOpId(), { barcode: code })))
      })
  }

  const cancelAsk = () => {
    if (ask) dropEdit(ask.line.product_id)
    setAsk(null)
  }

  const counted = allLines.filter((l) => l.counted_qty !== null)
  /** Посчитано кем угодно — для проведения, в каком бы виде владелец ни смотрел. */
  const countedAll = rawLines.filter((l) => l.counted_qty !== null)
  const unsent = unsentCount(queue, fresh)
  const unsentLines = unsentByLine(queue, fresh)
  const codes = unsentCodes(queue, fresh)
  // Ревизию провели или отменили, а сканы с устройства ещё не дошли: сколько не учтено — одним числом.
  const closedCount = closedLost + (draft ? 0 : queue.length)
  const sendNow = (
    <Button variant="secondary" onClick={() => void flush()}>
      Отправить сейчас
    </Button>
  )
  const codesOf = (why: 'not_found' | 'not_fit' | 'need_qty') => [...new Set(lostCodes.filter((c) => c.why === why).map((c) => c.code))].join(', ')
  const diffs = counted.filter((l) => l.counted_qty !== l.expected_qty)
  const overs = counted.filter((l) => overNorm(l, rev.oil_norm_bp))
  const valueTotal = rev.lines.reduce((acc, l) => acc + (l.value_delta_tyiyn ?? 0), 0)

  return (
    <div className="flex flex-col gap-4">
      {closedCount > 0 && (
        <Notice tone="rose">
          <div className="flex flex-wrap items-center gap-3">
            <span data-testid="unsent-closed">
              {closedText(closedCount, rev.head.number, rev.head.status === 'cancelled' ? 'cancelled' : 'posted')} — найденное проведите
              оприходованием или новой ревизией.
            </span>
            <Button
              variant="secondary"
              onClick={() => {
                setClosedLost(0)
                if (!draft) putQueue(() => [])
              }}
            >
              Понятно
            </Button>
          </div>
        </Notice>
      )}
      {(lostNotes.length > 0 || lostCodes.length > 0) && (
        <Notice tone="rose">
          <div className="flex flex-wrap items-center gap-3">
            <div data-testid="unsent-lost" className="flex flex-col gap-1">
              {codesOf('not_found') && (
                <span>
                  Сканы без связи не учтены — товар не найден по кодам: {codesOf('not_found')}. Заведите его кнопкой «+ Товар не из программы».
                </span>
              )}
              {codesOf('not_fit') && (
                <span>
                  Сканы без связи не учтены — товар не входит в эту ревизию
                  {rev.head.category_name ? ` (категория «${rev.head.category_name}»)` : ''}, коды: {codesOf('not_fit')}.
                </span>
              )}
              {codesOf('need_qty') && (
                <span>Сканы без связи не учтены — товар на вес или масло без тары, впишите количество в строке; коды: {codesOf('need_qty')}.</span>
              )}
              {lostNotes.map((t) => (
                <span key={t}>{t}</span>
              ))}
            </div>
            <Button
              variant="secondary"
              onClick={() => {
                setLostNotes([])
                setLostCodes([])
              }}
            >
              Понятно
            </Button>
          </div>
        </Notice>
      )}
      {draft && unsent > 0 && (
        <Notice>
          <div className="flex flex-wrap items-center gap-3">
            <div className="flex flex-col gap-1">
              <span data-testid="unsent">
                ⏳ {unsentText(unsent)}. Сканы не потеряны — повторно не сканируйте.
              </span>
              {codes.map((c) => (
                <span key={c} data-unsent-code={c}>
                  ⏳ скан {c} — товар уточнится при связи
                </span>
              ))}
              {needLogin && <span data-testid="unsent-login">Войдите снова — {scansText(unsent)} {plural(unsent, ['ждёт', 'ждут', 'ждут'])} отправки.</span>}
            </div>
            {sendNow}
          </div>
        </Notice>
      )}
      {others.map((o) => (
        <Notice key={o.user}>
          <span data-testid="unsent-other">
            На этом устройстве {o.count} {plural(o.count, ['неотправленный скан', 'неотправленных скана', 'неотправленных сканов'])} пользователя {o.user}{' '}
            — они уйдут, когда он войдёт и откроет эту ревизию.
          </span>
        </Notice>
      ))}
      {draft && (
        <Card className="flex flex-col gap-3">
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label="Сканер" hint="Скан прибавляет 1 штуку или 1 канистру; початую тару впишите литрами в строке">
              <div className="flex gap-2">
                <input
                  autoFocus
                  data-picker
                  className="min-w-0 flex-1"
                  value={scan}
                  onChange={(e) => setScan(e.target.value)}
                  onKeyDown={(e) => e.key === 'Enter' && onScan()}
                  placeholder="Сканируйте штрихкод"
                />
                {/* Камера телефона читает код так же, как сканер (задача 007). */}
                <CameraScanButton onCode={(code) => onScan(code)} />
              </div>
            </Field>
            <Field label="Найти в списке">
              <input value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="Название, артикул или вязкость" />
            </Field>
          </div>
          <div>
            <Button variant="secondary" onClick={() => setAdding({})}>
              + Товар не из программы
            </Button>
          </div>
          <div className="text-sm text-slate-600">
            {owner && (
              <div className="mb-2 flex gap-1" role="group" aria-label="Чей пересчёт показывать">
                {(
                  [
                    [false, 'Моё'],
                    [true, 'Все вместе'],
                  ] as const
                ).map(([v, label]) => (
                  <button
                    key={label}
                    type="button"
                    aria-pressed={allView === v}
                    data-view={v ? 'all' : 'own'}
                    onClick={() => setAllView(v)}
                    className={`min-h-[36px] rounded border px-3 text-sm ${allView === v ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white text-slate-600 hover:bg-slate-50'}`}
                  >
                    {label}
                  </button>
                ))}
              </div>
            )}
            {ownView ? (
              <>
                Вы посчитали товаров: {counted.length}
                {!blind && <>, расхождений {diffs.length}</>}. В списке — только то, что посчитали вы: сканируйте или найдите товар в поиске («Посчитать»); чужие числа не видны; итог по каждой строке
                складывается из пересчёта всех. Число в строке — «+ добавить»: прибавится к вашему. Пробили лишнее — «= моё» и впишите,
                сколько нашли вы. 0 в непосчитанной строке — «не нашли».
                {owner && ' Итог всех и проведение — «Все вместе».'}
              </>
            ) : (
              <>
                Пересчитано {counted.length} из {allLines.length}, расхождений {diffs.length}
                {overs.length > 0 && <span className="font-medium text-rose-700">, масло сверх нормы {percentText(rev.oil_norm_bp)}: {overs.length}</span>}.
                Непересчитанные товары не меняются. Число в строке — «+ добавить»: то, что вы нашли в своём месте, прибавится к общему
                итогу (считают несколько человек со своих входов — итог складывается). 0 в непосчитанной строке — «не нашли». Записать итог строки
                целиком — выберите «= итог».
              </>
            )}
          </div>
        </Card>
      )}
      {draft && (
        <RevisionSearchHits
          text={filter}
          version={addedCount}
          shown={new Set(shownLines.map((l) => l.product_id))}
          fits={fitsRevision}
          mayUmag={mayUmag}
          onCount={goTo}
          onUmag={(hit) => setAdding({ hit })}
        />
      )}
      {viscOptions.length > 0 && (
        <label className="flex items-center gap-2 self-start text-sm text-slate-600">
          Вязкость:
          <select id="revision-viscosity" value={visc} onChange={(e) => setVisc(e.target.value)}>
            <option value="">все</option>
            {viscOptions.map(([key, v]) => (
              <option key={key} value={key}>
                {v}
              </option>
            ))}
          </select>
        </label>
      )}
      <div className={`flex justify-end ${blind ? 'hidden' : ''}`}>
        {/* Все строки ревизии по отбору; «В деньгах» — только владельцу, как в таблице (задача 003 Н13, I13). */}
        <ExportButton
          file={`Ревизия ${rev.head.number}`}
          disabled={lines.length === 0}
          sheets={() => {
            const cols = revisionCols(owner && !draft, rev.oil_norm_bp)
            return [
              listSheet(`Ревизия № ${rev.head.number}: ${rev.head.category_name ?? 'весь склад'}`, cols, lines, {
                sheet: 'Ревизия',
                filter: [q && `поиск «${filter.trim()}»`, visc && `вязкость ${viscs.get(visc) ?? visc}`, `статус: ${STATUS[rev.head.status][0]}`],
                total: owner && !draft ? totalBy(cols, { Товар: 'Итого', 'В деньгах': xl.money(valueTotal) }) : undefined,
              }),
            ]
          }}
        />
      </div>
      <ErrorBox error={act.error} />
      <Card className="p-0">
        {lines.length === 0 ? (
          <Empty>Товаров нет</Empty>
        ) : (
          <Table head={blind ? ['Товар', 'Пересчитано вами'] : ['Товар', 'Должно быть', ownView ? 'Пересчитано вами' : 'Пересчитано', 'Разница', ...(owner && !draft ? ['В деньгах'] : [])]}>
            {pager.shown.map((l) => {
              const diff = l.counted_qty === null ? null : l.counted_qty - l.expected_qty
              const edit = edits[l.product_id]
              const tare = oilTare(l)
              const mode = fitMode(modes[l.product_id])
              const v = viscosityOf(l)
              return (
                <tr key={l.product_id}>
                  <td className="px-2 py-2">
                    <div className="font-medium">
                      <button
                        type="button"
                        className="text-left font-medium text-sky-800 hover:underline"
                        data-testid="revision-card"
                        title="Открыть карточку товара"
                        onClick={() =>
                          void act.run(async () => {
                            setCard(await get<Product>(`/products/${l.product_id}`))
                          })
                        }
                      >
                        {l.name}
                      </button>{' '}
                      {v &&
                        (v.fromName ? (
                          <span className="text-xs font-normal text-slate-400" data-testid="viscosity-from-name" title="Вязкость из названия — заполните в карточке">
                            {v.v} · из названия — заполните в карточке
                          </span>
                        ) : (
                          <span data-testid="viscosity">
                            <Badge tone="sky">{v.v}</Badge>
                          </span>
                        ))}
                    </div>
                    {l.article && <div className="text-xs text-slate-500">{l.article}</div>}
                  </td>
                  {!blind && (
                    <td className="whitespace-nowrap px-2 py-2" data-testid="expected">
                      {countText(l, l.expected_qty)}
                    </td>
                  )}
                  <td className="whitespace-nowrap px-2 py-2">
                    {draft ? (
                      <div className="flex flex-col gap-1">
                        <div className="flex gap-1" role="group" aria-label="Как записать">
                          {(ownView
                            ? ([
                                ['add', '+ добавить', 'Прибавить найденное к своему пересчёту'],
                                ['mine', '= моё', 'Исправить своё число строки: сколько нашли вы'],
                              ] as const)
                            : ([
                                ['add', '+ добавить', 'Прибавить найденное в своём месте к общему итогу'],
                                ['set', '= итог', 'Записать итог строки целиком'],
                              ] as const)
                          ).map(([m, label, title]) => (
                            <button
                              key={m}
                              type="button"
                              title={title}
                              aria-pressed={mode === m}
                              data-mode={m}
                              // Поле не теряет фокус: набранное не записывается в прежнем режиме.
                              onMouseDown={(e) => e.preventDefault()}
                              onClick={() => switchMode(l, m)}
                              className={`min-h-[28px] rounded border px-2 text-xs ${mode === m ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white text-slate-600 hover:bg-slate-50'}`}
                            >
                              {label}
                            </button>
                          ))}
                        </div>
                        {tare ? (
                          <OilCountInput
                            containerMl={tare}
                            dataCount={l.product_id}
                            value={
                              oilEdits[l.product_id] ??
                              (mode === 'add' || l.counted_qty === null ? { cans: '', liters: '' } : oilFields(l.counted_qty, tare))
                            }
                            onChange={(val) => putOilEdit(l, val)}
                            onCommit={() => saveOil(l, tare)}
                          />
                        ) : (
                          <input
                            data-count={l.product_id}
                            className="w-24"
                            inputMode={l.unit === 'piece' ? 'numeric' : 'decimal'}
                            placeholder={`${mode === 'add' ? '+ ' : ''}${l.unit === 'ml' ? 'л' : l.unit === 'g' ? 'кг' : 'шт'}`}
                            aria-label={l.unit === 'ml' ? 'Пересчитано, л' : l.unit === 'g' ? 'Пересчитано, кг' : 'Пересчитано, шт'}
                            value={edit ?? (mode === 'add' || l.counted_qty === null ? '' : countInput(l, l.counted_qty))}
                            onChange={(e) => putEdit(l, e.target.value)}
                            onKeyDown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()}
                            onBlur={() => {
                              const typed = editsNow.current[l.product_id]
                              if (typed === undefined) return
                              const v = parseCount(l, typed)
                              if (typed.trim() && v === null) {
                                act.setError(`Неверное количество: ${l.name}`)
                                return
                              }
                              commit(l, v)
                            }}
                          />
                        )}
                        {l.counted_qty !== null && (
                          <div className="text-xs font-medium text-slate-700" data-total={l.product_id}>
                            {ownView ? 'моё' : 'итого'} {countText(l, l.counted_qty)}
                          </div>
                        )}
                        {unsentLines.has(l.product_id) && (
                          <div
                            className="text-xs font-medium text-amber-700"
                            data-unsent={l.product_id}
                            title="Скан не дошёл до сервера — отправится сам, когда появится связь"
                          >
                            ⏳ +{countText(l, unsentLines.get(l.product_id) ?? 0)} не отправлено
                          </div>
                        )}
                      </div>
                    ) : l.counted_qty === null ? (
                      '—'
                    ) : (
                      countText(l, l.counted_qty)
                    )}
                    {!ownView && l.counted_qty !== null && (l.counted_log?.length || l.counted_by) && (
                      <div className="mt-0.5 whitespace-normal text-xs text-slate-400" data-testid="counted-by">
                        {l.counted_log?.length ? (
                          // Кто сколько добавил к итогу; весь журнал — подсказкой.
                          <span title={`Весь журнал: ${countLogText(l.counted_log, (n) => countText(l, n))}`}>
                            {countSummary(l.counted_log, l.counted_qty, (n) => countText(l, n))}
                          </span>
                        ) : (
                          `последним считал: ${l.counted_by}`
                        )}
                      </div>
                    )}
                  </td>
                  <td hidden={blind} className={`whitespace-nowrap px-2 py-2 font-medium ${diff === null || diff === 0 ? 'text-slate-400' : diff < 0 ? 'text-rose-700' : 'text-emerald-700'}`}>
                    {diff === null ? '—' : diff === 0 ? 'сходится' : `${diff > 0 ? '+' : '−'}${countText(l, Math.abs(diff))}`}
                    {l.unit === 'ml' && diff !== null && diff !== 0 && (
                      <div className="mt-0.5">
                        {overNorm(l, rev.oil_norm_bp) ? <Badge tone="rose">сверх нормы</Badge> : <Badge tone="slate">в норме</Badge>}
                      </div>
                    )}
                  </td>
                  {owner && !draft && (
                    <td className="whitespace-nowrap px-2 py-2">{l.value_delta_tyiyn ? formatSom(l.value_delta_tyiyn) : '—'}</td>
                  )}
                </tr>
              )
            })}
          </Table>
        )}
        <div className="px-4 pb-3 empty:hidden">
          <Pager page={pager.page} size={pager.size} total={pager.total} onPage={pager.setPage} onSize={pager.setSize} label="Страницы товаров ревизии" />
        </div>
      </Card>
      {owner && !draft && rev.head.status === 'posted' && (
        <div className="text-right font-semibold">Итог ревизии в деньгах: {formatSom(valueTotal)}</div>
      )}
      {draft && (
        <div className="flex flex-wrap justify-end gap-2">
          <Button
            variant="secondary"
            disabled={act.busy}
            onClick={() =>
              void act.run(async () => {
                await post(`/revisions/${rev.head.id}/cancel`)
                onChanged()
              })
            }
          >
            Отменить ревизию
          </Button>
          {owner ? (
            <Button disabled={act.busy || countedAll.length === 0} onClick={() => setPosting({ comment: '', opId: newOpId() })}>
              Провести
            </Button>
          ) : (
            <span className="self-center text-sm text-slate-500">Провести ревизию может владелец</span>
          )}
        </div>
      )}
      {adding && (
        <RevisionNewProduct
          revisionId={rev.head.id}
          categoryId={rev.head.category_id}
          presetBarcode={adding.barcode}
          presetHit={adding.hit}
          onClose={() => setAdding(null)}
          onExisting={goTo}
          onDone={() => {
            setAdding(null)
            setAddedCount((n) => n + 1)
            onChanged()
          }}
        />
      )}
      {card && (
        // Остаток в карточке не правится — только пересчётом ревизии (I6); после закрытия строка
        // перечитывается: название и коды могли поменяться.
        <ProductFormModal
          product={card}
          inRevision
          onClose={() => {
            setCard(null)
            onChanged()
          }}
          onSaved={() => {
            setCard(null)
            onChanged()
          }}
        />
      )}
      {ask && (
        <Modal title="Пока вы вводили, итог изменился" onClose={cancelAsk}>
          <div className="flex flex-col gap-3">
            <div className="text-sm text-slate-700" data-testid="count-changed">
              «{ask.line.name}»: пока вы вводили, итог изменили — сейчас{' '}
              <b>{ask.now === null ? 'строка не посчитана' : countText(ask.line, ask.now)}</b>.{' '}
              {ask.qty === null ? 'Всё равно убрать строку?' : `Записать итог ${countText(ask.line, ask.qty)}?`}
            </div>
            <div className="flex flex-wrap justify-end gap-2">
              <Button variant="secondary" onClick={cancelAsk}>
                Отмена
              </Button>
              {ask.qty !== null && ask.qty > 0 && (
                <Button
                  variant="secondary"
                  onClick={() => {
                    setAsk(null)
                    // Поле переходит в «+ добавить» с тем же числом: ответ потеряли — Enter в поле
                    // уходит тем же op_id, а не прибавляет второй раз; записано — поле очищается.
                    const id = ask.line.product_id
                    putMode(id, 'add')
                    save(ask.line, ask.qty, 'add', ops.take(id, inputKey('add', ask.qty)))
                  }}
                >
                  Прибавить {countText(ask.line, ask.qty)} — будет {countText(ask.line, (ask.now ?? 0) + ask.qty)}
                </Button>
              )}
              <Button
                onClick={() => {
                  setAsk(null)
                  // Человек увидел «сейчас N» и подтвердил: сверка — с ним.
                  own.see(ask.line.product_id, ask.now)
                  save(ask.line, ask.qty, 'set', ops.take(ask.line.product_id, inputKey('set', ask.qty)))
                }}
              >
                {ask.qty === null ? 'Убрать строку' : `Записать итог ${countText(ask.line, ask.qty)}`}
              </Button>
            </div>
          </div>
        </Modal>
      )}
      {posting && (
        <Modal title={`Провести ревизию № ${rev.head.number}`} onClose={() => setPosting(null)}>
          <div className="flex flex-col gap-3">
            <div className="text-sm text-slate-600">
              Остатки {counted.length} товаров станут равны пересчитанным, расхождений — {diffs.length}. Недостача уменьшит прибыль по средней
              себестоимости. Проведённую ревизию не изменить.
            </div>
            <Field label="Кто пересчитывал, причина" required>
              <input autoFocus value={posting.comment} onChange={(e) => setPosting({ ...posting, comment: e.target.value })} />
            </Field>
            {unsent > 0 && (
              <Notice tone="rose">
                <div className="flex flex-wrap items-center gap-3">
                  <span data-testid="unsent-on-post">
                    На этом устройстве не отправлено сканов: {unsent} — без связи они не войдут в ревизию. Отправьте их перед проведением.
                  </span>
                  {sendNow}
                </div>
              </Notice>
            )}
            {posting.warn && (
              <Notice>
                <span data-testid="still-counting">{posting.warn}</span> Скан, который придёт после проведения, не учтётся.
              </Notice>
            )}
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setPosting(null)}>
                Отмена
              </Button>
              <Button
                disabled={act.busy || !posting.comment.trim()}
                onClick={() =>
                  void act.run(async () => {
                    try {
                      await post(`/revisions/${rev.head.id}/post`, { op_id: posting.opId, comment: posting.comment, force: Boolean(posting.warn) })
                    } catch (e) {
                      // Другие ещё считают (записи за последние 60 с) — спросить, проводить ли.
                      if (e instanceof ApiError && e.code === 'still_counting') {
                        setPosting((p) => (p ? { ...p, warn: e.message } : p))
                        return
                      }
                      throw e
                    }
                    setPosting(null)
                    onChanged()
                    toast('Ревизия проведена')
                  })
                }
              >
                {posting.warn ? 'Провести всё равно' : 'Провести'}
              </Button>
            </div>
          </div>
        </Modal>
      )}
    </div>
  )
}

/** Один экран: ревизии и сканер товаров («Работа с товарами»). Вкладка — в адресе (`?tab=scan`). */
export default function RevisionAndGoods() {
  const [params, setParams] = useSearchParams()
  // Ревизия — право «Ревизия»; сканер товаров — для заведения товаров или пересчёта (ADR-097).
  const can = useCan()
  const tabs = (
    [
      ['revision', 'Ревизия'],
      ['scan', 'Сканер товаров'],
    ] as const
  ).filter(([key]) => (key === 'revision' ? can('revision') : can('products') || can('revision')))
  const tab = params.get('tab') === 'scan' || !can('revision') ? 'scan' : 'revision'
  return (
    <div className="flex flex-col gap-4">
      <div className="flex gap-2" role="tablist" aria-label="Ревизия и товары">
        {tabs.map(([key, label]) => (
          <button
            key={key}
            type="button"
            role="tab"
            aria-selected={tab === key}
            className={`min-h-[38px] rounded-md border px-4 py-2 text-sm font-medium ${tab === key ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white hover:bg-slate-50'}`}
            onClick={() => setParams(key === 'scan' ? { tab: 'scan' } : {}, { replace: true })}
          >
            {label}
          </button>
        ))}
      </div>
      {tab === 'scan' ? <ProductIntake /> : <Revision />}
    </div>
  )
}

function Revision() {
  const list = useLoad(() => get<Head[]>('/revisions'), [])
  const listPager = usePager(list.data ?? [], 'revisions')
  const categories = useLoad(() => get<Category[]>('/categories'), [])
  const [openId, setOpenId] = useState<string | null>(null)
  const rev = useLoad(() => (openId ? get<RevisionOut>(`/revisions/${openId}`) : Promise.resolve(null)), [openId])
  const [category, setCategory] = useState('')
  const act = useAction()
  const owner = useUser().role === 'owner'
  const settings = useLoad(() => get<{ stale_days: number; oil_norm_bp: number }>('/settings/stock'), [])
  const [norm, setNorm] = useState<string | null>(null)
  const normBp = norm === null ? null : parsePercentBp(norm)

  /** Открытый черновик другого охвата: «весь склад» при открытой категории — присоединиться к ней. */
  const [busy, setBusy] = useState<{ id: string; message: string } | null>(null)

  // Ответы записи и перечитывания приходят в любом порядке: строка показывается из ответа, только
  // если он новее показанной (версия строки — номер её последней записи на сервере), — старый
  // итог не возвращается поверх свежего (задача 003, Н14).
  const setRev = rev.setData
  const openNow = useRef(openId)
  useEffect(() => {
    openNow.current = openId
  }, [openId])

  // Очереди неотправленных сканов на устройстве: проведённых и отменённых ревизий — убираются со
  // словами, сколько сканов не учтено; сканы старше недели — убираются (задача 003, Н14).
  const [lostClosed, setLostClosed] = useState<{ revisionId: string; number: number; status: 'posted' | 'cancelled'; count: number }[]>([])
  useEffect(() => {
    if (!list.data) return
    const closed = new Map(
      list.data
        .filter((h) => h.status !== 'draft' && h.id !== openNow.current)
        .map((h) => [h.id, { number: h.number, status: h.status === 'posted' ? ('posted' as const) : ('cancelled' as const) }]),
    )
    void withQueueLock(browserLocks(), () => cleanQueues(localStorage, closed)).then((lost) => {
      if (lost.length > 0) setLostClosed((was) => [...was.filter((x) => !lost.some((y) => y.revisionId === x.revisionId)), ...lost])
    })
  }, [list.data])
  const onFresh = useCallback((update: (r: RevisionOut) => RevisionOut) => setRev((r) => (r ? update(r) : r)), [setRev])
  const reloadRev = useCallback(async () => {
    const id = openNow.current
    if (!id) return
    try {
      const r = await get<RevisionOut>(`/revisions/${id}`)
      if (openNow.current !== id) return
      // Проведённая или отменённая ревизия в черновик не возвращается: ответ перечитывания,
      // ушедшего до проведения, мог прийти позже.
      setRev((cur) => freshRevision(cur, r))
    } catch {
      // Следующее перечитывание покажет.
    }
  }, [setRev])

  const start = () =>
    void act.run(async () => {
      setBusy(null)
      const want = category || null
      let r: RevisionOut
      try {
        // Двое нажали «Начать ревизию» — один черновик на всех; открыт «весь склад» — категория
        // считается в нём: две ревизии одного товара вычли бы остаток дважды (задача 003, Н14).
        r = await post<RevisionOut>('/revisions', { category_id: want })
      } catch (e) {
        if (e instanceof ApiError && e.code === 'revision_open') {
          const open = (await get<Head[]>('/revisions')).find((h) => h.status === 'draft' && h.category_id !== null)
          if (open) {
            setBusy({ id: open.id, message: e.message })
            return
          }
        }
        throw e
      }
      list.reload()
      if (r.head.category_id !== want) toast('Идёт ревизия всего склада — вы присоединились к ней')
      setOpenId(r.head.id)
    })

  if (openId && rev.data) {
    const h = rev.data.head
    return (
      <div className="flex flex-col gap-4">
        <PageHeader
          title={`Ревизия № ${h.number}${h.category_name ? ` · ${h.category_name}` : ' · весь склад'}`}
          actions={
            <Button
              variant="secondary"
              onClick={() => {
                setOpenId(null)
                list.reload()
              }}
            >
              К списку
            </Button>
          }
        />
        <Draft
          key={h.id}
          rev={rev.data}
          onChanged={() => {
            void reloadRev()
            list.reload()
          }}
          onFresh={onFresh}
          reload={reloadRev}
        />
      </div>
    )
  }

  return (
    <div className="flex flex-col gap-4">
      <PageHeader title="Ревизия" />
      <Card className="flex flex-wrap items-end gap-3">
        <Field label="Что пересчитываем">
          <select value={category} onChange={(e) => setCategory(e.target.value)}>
            <option value="">Весь склад</option>
            {(categories.data ?? []).map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </select>
        </Field>
        <Button disabled={act.busy} onClick={start}>
          Начать ревизию
        </Button>
        <div className="text-xs text-slate-500">
          Раз в месяц: пересчитайте сканером, владелец проведёт и остатки выровняются. Начали несколько человек — все считают в одной ревизии.
        </div>
      </Card>
      {lostClosed.length > 0 && (
        <Notice tone="rose">
          <div className="flex flex-wrap items-center gap-3">
            <div data-testid="unsent-closed" className="flex flex-col gap-1">
              {lostClosed.map((x) => (
                <span key={x.revisionId}>
                  На этом устройстве оставались неотправленные сканы: {closedText(x.count, x.number, x.status)} — найденное проведите
                  оприходованием или новой ревизией.
                </span>
              ))}
            </div>
            <Button variant="secondary" onClick={() => setLostClosed([])}>
              Понятно
            </Button>
          </div>
        </Notice>
      )}
      {busy && (
        <Notice>
          <div className="flex flex-wrap items-center gap-3">
            <span data-testid="revision-busy">{busy.message}</span>
            <Button
              variant="secondary"
              onClick={() => {
                setOpenId(busy.id)
                setBusy(null)
              }}
            >
              Присоединиться
            </Button>
          </div>
        </Notice>
      )}
      {settings.data &&
        (norm === null ? (
          <div className="-mt-2 px-1 text-sm text-slate-600">
            <span title="Расхождение по маслу больше нормы помечается «сверх нормы», владельцу приходит уведомление">
              Норма по маслу: <span className="font-medium text-slate-800">{percentText(settings.data.oil_norm_bp)}</span>
            </span>
            {owner && (
              <>
                {' · '}
                <button
                  type="button"
                  className="text-sky-700 underline decoration-dotted hover:text-sky-800"
                  onClick={() => setNorm(String(settings.data!.oil_norm_bp / 100).replace('.', ','))}
                >
                  изменить
                </button>
              </>
            )}
          </div>
        ) : (
          <Card className="flex flex-wrap items-end gap-3 text-sm">
            <div className="w-full text-slate-600">
              Норма расхождения по маслу — от того, что должно быть. Больше — строка ревизии помечается «сверх нормы», владельцу приходит
              уведомление.
            </div>
            <Field label="Норма по маслу, %" hint="От 0 до 10, например 0,5">
              <input autoFocus inputMode="decimal" className="w-24" value={norm} onChange={(e) => setNorm(e.target.value)} />
            </Field>
            <Button variant="secondary" onClick={() => setNorm(null)}>
              Отмена
            </Button>
            <Button
              disabled={act.busy || normBp === null || normBp > 1000}
              onClick={() =>
                void act.run(async () => {
                  await put('/settings/stock', { oil_norm_bp: normBp })
                  setNorm(null)
                  settings.reload()
                  toast('Норма сохранена')
                })
              }
            >
              Сохранить
            </Button>
          </Card>
        ))}
      <div className="flex justify-end">
        <ExportButton file="Ревизии" disabled={(list.data ?? []).length === 0} sheets={() => [listSheet('Ревизии', REVISION_LIST_COLS, list.data ?? [])]} />
      </div>
      <ErrorBox error={list.error ?? act.error ?? rev.error} />
      <Card className="p-0">
        {list.loading && !list.data ? (
          <Loading />
        ) : (list.data ?? []).length === 0 ? (
          <Empty
            icon="revision"
            action={
              <Button disabled={act.busy} onClick={start}>
                Начать первую ревизию
              </Button>
            }
          >
            Ревизий ещё не было. Выберите, что пересчитываем, и начните — пересчёт идёт сканером.
          </Empty>
        ) : (
          <Table head={['№', 'Что', 'Начал', 'Пересчитано', 'Статус', '']}>
            {listPager.shown.map((h) => (
              <tr key={h.id} className="cursor-pointer hover:bg-slate-50" onClick={() => setOpenId(h.id)}>
                <td className="px-2 py-2 font-medium">{h.number}</td>
                <td className="px-2 py-2">{h.category_name ?? 'Весь склад'}</td>
                <td className="whitespace-nowrap px-2 py-2">
                  {h.user_name}, {formatDateTime(h.created_at)}
                </td>
                <td className="px-2 py-2">{h.counted}</td>
                <td className="px-2 py-2">
                  <Badge tone={STATUS[h.status][1]}>{STATUS[h.status][0]}</Badge>
                </td>
                <td className="px-2 py-2 text-right text-sky-700">Открыть</td>
              </tr>
            ))}
          </Table>
        )}
        <div className="px-4 pb-3 empty:hidden">
          <Pager page={listPager.page} size={listPager.size} total={listPager.total} onPage={listPager.setPage} onSize={listPager.setSize} label="Страницы ревизий" />
        </div>
      </Card>
    </div>
  )
}

/** Строки ревизии: количества числами в единице товара; «В деньгах» — только владельцу после проведения. */
function revisionCols(money: boolean, normBp: number): ExportCol<Line>[] {
  return [
    { label: 'Товар', value: (l) => l.name },
    { label: 'Артикул', value: (l) => l.article },
    { label: 'Вязкость', value: (l) => viscosityOf(l)?.v ?? '' },
    { label: 'Ед.', value: (l) => unitName(l.unit) },
    { label: 'Должно быть', value: (l) => xl.units(l.expected_qty, l.unit) },
    { label: 'Пересчитано', value: (l) => (l.counted_qty === null ? null : xl.units(l.counted_qty, l.unit)) },
    { label: 'Разница', value: (l) => (l.counted_qty === null ? null : xl.units(l.counted_qty - l.expected_qty, l.unit)) },
    {
      label: 'Норма',
      value: (l) => (l.unit === 'ml' && l.counted_qty !== null && l.counted_qty !== l.expected_qty ? (overNorm(l, normBp) ? 'сверх нормы' : 'в норме') : ''),
    },
    ...colsIf<Line>(money, [{ label: 'В деньгах', value: (l) => xl.money(l.value_delta_tyiyn ?? 0) }]),
  ]
}

const REVISION_LIST_COLS: ExportCol<Head>[] = [
  { label: '№', value: (h) => xl.int(h.number) },
  { label: 'Что', value: (h) => h.category_name ?? 'Весь склад' },
  { label: 'Начал', value: (h) => h.user_name },
  { label: 'Когда', value: (h) => xl.dateTime(h.created_at) },
  { label: 'Проведена', value: (h) => xl.dateTime(h.posted_at) },
  { label: 'Пересчитано', value: (h) => xl.int(h.counted) },
  { label: 'Статус', value: (h) => STATUS[h.status][0] },
  { label: 'Комментарий', value: (h) => h.comment },
]
