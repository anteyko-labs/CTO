// Новый приход: сканер, создание незнакомых товаров, цены из прошлого прихода, печать этикеток (SPEC-03).
// Строка как в старой программе: цена по накладной, наценка %, цена продажи, сумма; наценка и цена
// продажи связаны, новая цена продажи ставится товару при проведении (SPEC-28 §1, ADR-075). Внизу
// итог по закупке и по продаже; при выборе товара — «в прошлый раз: цена, дата, поставщик» (SPEC-27).
// Ввод подряд с клавиатуры и подсказки из старой программы, часть оплаты сразу, фото накладной (SPEC-29).
// Вверху — вид документа: «От поставщика» или «Оприходование (без поставщика)» (SPEC-29 §8, ADR-086):
// те же строки, но без поставщика и оплаты, с обязательным комментарием; количество — как на складе.
// Накладная по фото (SPEC-30 §9): черновик из бота или «Заполнить по фото» заполняет приход —
// поставщик, номер, строки (найденные товары, «проверьте», не найденные); итог накладной сверяется
// с суммой строк; при проведении фото прикрепляется к накладной.
import { useEffect, useState, type KeyboardEvent } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { CorrectionBanner } from '../components/Correction'
import { printLabels } from '../components/labels'
import { ProductFormModal } from '../components/ProductFormModal'
import { UnknownCodeModal } from '../components/UnknownCodeModal'
import { ProductPicker, stockText, type PickHow } from '../components/ProductPicker'
import { PendingPhotos, PhotoButtons } from '../components/ReceiptPhotos'
import { DraftThumb, FillFromPhoto, ReceiptDraftsBlock } from '../components/ReceiptDrafts'
import { preparePhotos, uploadPhotos, type PendingPhoto } from '../lib/photoUpload'
import { docText, splitLines, suggestFor, totalMismatch, type Draft, type DraftLine, type DraftList } from '../lib/receiptDrafts'
import { Badge, Button, Card, CardTitle, Checkbox, Empty, ErrorBox, Field, Missing, Notice, PageHeader, Table, toast } from '../components/ui'
import { accountLabel } from '../lib/accounts'
import { get, newOpId, post } from '../lib/api'
import { postCorrection } from '../lib/corrections'
import type { ReceiptFixState } from '../lib/receiptFix'
import { formatLiters, formatSom, litersInput, parseSom, somInput } from '../lib/format'
import {
  defaultMode,
  firstCount,
  initialPrices,
  isOil,
  lastPer,
  lastText,
  lineCalc,
  literHints,
  literTare,
  nextCount,
  priceJumpText,
  switchMode,
  withCost,
  withMarkup,
  withSalePrice,
  type QtyMode,
} from '../lib/receiptLines'
import { kgInput, perLiter } from '../lib/units'
import { missingWithFocus } from '../lib/forms'
import { useFocusAfterRender } from '../lib/focus'
import { useUser } from '../lib/auth'
import { useAction, useLoad } from '../lib/hooks'
import { nextField, type LineField } from '../lib/receiptKeys'
import { defaultPaySource, notEnough, paidNow, partProblem, paySources, type PayMode } from '../lib/receiptPay'
import { useCan } from '../lib/permissions'
import type { CashAccount, LastReceipt, Product, Receipt, StockIn, Supplier } from '../lib/types'
import { useUmagOn } from '../lib/umagSwitch'

/** Вид документа на экране: приход от поставщика или оприходование без поставщика. */
export type ReceiptKind = 'receipt' | 'stock_in'

interface Line {
  key: string
  product: Product
  /** Количество: штуки, канистры или литры масла (по режиму), кг на вес. */
  count: string
  /** Масло: канистрами или литрами (SPEC-36 §13); у остальных не задан. */
  mode?: QtyMode
  /** Цена по накладной за штуку, канистру, литр (режим «л») или кг. */
  price: string
  /** Наценка над ценой по накладной, %, с одним знаком. */
  markup: string
  /** Цена продажи за штуку или канистру; по умолчанию — текущая цена товара. */
  salePrice: string
  /** Прошлая закупка: undefined — ещё грузится, null — закупок не было. */
  last?: LastReceipt | null
  /** Строка из накладной по фото: как в накладной, «проверьте» у похожего названия, пояснение. */
  hint?: { invoice: string; check: boolean; note: string | null }
}

/** Исправление прихода владельцем (SPEC-29 §24): тот же экран, заполненный исходной накладной. */
export interface ReceiptFix {
  /** Что исправляют: накладную или оприходование; по умолчанию — накладную. */
  kind?: ReceiptKind
  originalId: string
  number: number | null
  state: ReceiptFixState
}

/** Без прав «Приход» и «Оприходование» — только «Нет права», без формы (ADR-097). */
export default function ReceiptNew({ fix }: { fix?: ReceiptFix }) {
  const can = useCan()
  if (!fix && !can('receipts') && !can('stock_in')) {
    return (
      <div>
        <PageHeader title="Новый приход" />
        <Card>
          <div className="text-sm text-slate-700" data-testid="no-right">
            Нет права: приход товара и оприходование. Его выдаёт владелец в «Пользователях».
          </div>
        </Card>
      </div>
    )
  }
  return <ReceiptForm fix={fix} />
}

function ReceiptForm({ fix }: { fix?: ReceiptFix }) {
  const navigate = useNavigate()
  const [params, setParams] = useSearchParams()
  const [opId] = useState(newOpId)
  // Приход и оприходование — отдельные права администратора (ADR-097): виден только разрешённый вид.
  const can = useCan()
  const [kind, setKind] = useState<ReceiptKind>(fix?.kind ?? (params.get('kind') === 'stock_in' || !can('receipts') ? 'stock_in' : 'receipt'))
  const stockIn = kind === 'stock_in'
  // Выключенный Умаг (SPEC-36 §15) в подсказках не упоминается.
  const umagOn = useUmagOn()
  const calcOf = (l: Line) => lineCalc(l, stockIn)
  const suppliers = useLoad(() => get<Supplier[]>('/suppliers'), [])
  const [supplierId, setSupplierId] = useState(fix?.state.supplierId ?? '')
  const [supplierDoc, setSupplierDoc] = useState(fix?.state.supplierDoc ?? '')
  const [comment, setComment] = useState(fix?.state.comment ?? '')
  // Сразу целиком, часть сейчас и остаток в долг, всё в долг (SPEC-29 §4).
  const [payMode, setPayMode] = useState<PayMode>(fix?.state.payMode ?? 'paid')
  const [partText, setPartText] = useState(fix?.state.partText ?? '')
  // «Что и почему исправляете» — обязателен (SPEC-29 §24).
  const [fixComment, setFixComment] = useState('')
  const onDebt = payMode !== 'paid'
  const [photos, setPhotos] = useState<PendingPhoto[]>([])
  const photoAction = useAction()
  // Оплачена сразу — деньги уходят из выбранной кассы (ADR-062); администратору — только «Касса».
  const owner = useUser().role === 'owner'
  const accounts = useLoad(() => get<CashAccount[]>('/cash/accounts'), [])
  const sources = paySources(accounts.data ?? [], owner)
  const [paidFrom, setPaidFrom] = useState(fix?.state.paidFrom ?? '')
  const source = paidFrom || defaultPaySource(sources, owner)
  const [lines, setLines] = useState<Line[]>(fix?.state.lines ?? [])
  const [print, setPrint] = useState(false)
  const [unknownCode, setUnknownCode] = useState<string | null>(null)
  const [newProductCode, setNewProductCode] = useState<string | null>(null)
  const [newProductName, setNewProductName] = useState<string | null>(null)
  const [editingProduct, setEditingProduct] = useState<Product | null>(null)
  const labels = useAction()
  const [newSupplier, setNewSupplier] = useState<{ name: string; phone: string } | null>(null)
  const save = useAction()
  const supplierAction = useAction()
  // Накладная по фото (SPEC-30 §9): открытый черновик, строки без товара и строка, для которой
  // сейчас выбирают товар в поиске.
  const drafts = useLoad(() => (fix ? Promise.resolve<DraftList>({ enabled: false, items: [] }) : get<DraftList>('/receipt-drafts')), [])
  const [draft, setDraft] = useState<Draft | null>(null)
  const [unmatched, setUnmatched] = useState<DraftLine[]>([])
  const [target, setTarget] = useState<number | null>(null)
  const draftAction = useAction()

  /** Перейти к полю: выделить текст, чтобы ввод сразу заменил подставленное. Фокус ставится
   *  в той же отрисовке, где появилась строка, — без таймера, который перехватывал набор. */
  const focusAfterRender = useFocusAfterRender()
  const focusId = (id: string | null) => focusAfterRender(`#${id ?? 'receipt-picker'}`)

  /** Enter в поле строки — следующее поле, потом следующая строка, потом поиск (SPEC-29 §1). */
  const onFieldKey = (lineNo: number, field: LineField) => (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key !== 'Enter') return
    e.preventDefault()
    focusId(nextField(lineNo, field, lines.length))
  }

  /** Прошлая закупка: цена, дата и поставщик — подсказка рядом с ценой (SPEC-27). */
  const loadLast = (p: Product) =>
    void get<LastReceipt | null>(`/products/${p.id}/last-receipt`)
      .then((last) => setLines((ls) => ls.map((l) => (l.product.id === p.id ? { ...l, last } : l))))
      .catch(() => setLines((ls) => ls.map((l) => (l.product.id === p.id ? { ...l, last: null } : l))))

  /** Строка прихода из строки накладной: количество и цена накладной, цена продажи — текущая. */
  const draftLine = (p: Product, count: number | null, each: number | null, hint: Line['hint'], measure?: 'l' | 'kg'): Line => {
    // Литры, не делящиеся на канистры, — строка в режиме «л» с ценой литра (SPEC-36 §13).
    const mode: QtyMode | undefined = measure === 'l' ? 'l' : isOil(p) ? 'can' : undefined
    const tare = literTare(p, mode)
    const prices = initialPrices(p, tare)
    const priced = each === null ? prices : withCost(prices, somInput(each), tare)
    const text = count === null ? '' : measure === 'l' ? litersInput(count) : measure === 'kg' ? kgInput(count) : String(count)
    return { key: crypto.randomUUID(), product: p, count: text, ...(mode ? { mode } : {}), ...priced, hint }
  }

  /** Открыть черновик накладной по фото: поставщик, номер, строки (SPEC-30 §9). */
  const applyDraft = (d: Draft) => {
    if (d.status !== 'new') {
      draftAction.setError(
        d.status === 'used' ? `По этой накладной уже проведён приход № ${d.receipt_number ?? ''}` : 'Эту накладную по фото убрали',
      )
      return
    }
    setKind('receipt')
    setDraft(d)
    setTarget(null)
    if (d.supplier) setSupplierId(d.supplier.id)
    const doc = docText(d.invoice)
    if (doc) setSupplierDoc(doc)
    const { found, missing } = splitLines(d)
    const added = found.flatMap((l) => {
      if (!l.product) return []
      // Литры не целыми канистрами и товар на вес — по правилу экрана: литры или кг (SPEC-36 §13).
      const own = suggestFor(l, l.product)
      const s = own.measure ? own : { count: l.count, each: l.each_tyiyn, note: l.note }
      return [draftLine(l.product, s.count, s.each, { invoice: l.name, check: l.match === 'similar', note: s.note }, own.measure)]
    })
    setLines((ls) => [...ls, ...added])
    setUnmatched(missing)
    added.forEach((l) => loadLast(l.product))
  }

  const openDraft = (id: string) =>
    void draftAction.run(async () => {
      applyDraft(await get<Draft>(`/receipt-drafts/${id}`))
      setParams({ draft: id }, { replace: true })
    })

  // «Открыть» из «Прихода» и из бота: черновик в адресе (`?draft=`).
  const draftParam = fix ? null : params.get('draft')
  useEffect(() => {
    if (!draftParam) return
    let alive = true
    void draftAction.run(async () => {
      const d = await get<Draft>(`/receipt-drafts/${draftParam}`)
      if (alive) applyDraft(d)
    })
    return () => {
      alive = false
    }
    // Только при открытии экрана: черновик из адреса заполняет приход один раз.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  /** Товар для строки накладной без товара: количество и цена — из накладной (литры — в канистры). */
  const assign = (no: number, p: Product) => {
    const src = unmatched.find((l) => l.no === no)
    setTarget(null)
    if (!src) return
    const s = suggestFor(src, p)
    setUnmatched((us) => us.filter((l) => l.no !== no))
    setLines((ls) => [...ls, draftLine(p, s.count, s.each, { invoice: src.name, check: false, note: s.note }, s.measure)])
    loadLast(p)
    focusId(`line-${lines.length + 1}-count`)
  }

  const addProduct = (p: Product, how: PickHow = 'search', cost?: string) => {
    // Выбирают товар для строки накладной по фото — он становится этой строкой.
    if (target !== null) {
      assign(target, p)
      return
    }
    // Скан — фокус остаётся в поиске (сканируют подряд); выбор из списка — сразу к количеству строки.
    const at = lines.findIndex((l) => l.product.id === p.id)
    if (how === 'search') focusId(`line-${at < 0 ? lines.length + 1 : at + 1}-count`)
    if (!lines.some((l) => l.product.id === p.id)) loadLast(p)
    setLines((ls) => {
      const i = ls.findIndex((l) => l.product.id === p.id)
      if (i < 0) {
        // Цена подставляется из прошлого прихода, цена продажи — текущая; их можно изменить.
        // Карточка из старой программы — закупка оттуда, пока своих приходов нет.
        // Масло: в приходе — канистрами, в оприходовании — литрами (SPEC-36 §13).
        const mode = isOil(p) ? defaultMode(stockIn) : undefined
        const tare = literTare(p, mode)
        const prices = initialPrices(p, tare)
        // Закупка из Умага — за канистру; в режиме «л» — за литр.
        const umagCost = cost && tare ? somInput(perLiter(parseSom(cost) ?? 0, tare)) : cost
        const priced = umagCost && !prices.price ? withCost(prices, umagCost, tare) : prices
        return [...ls, { key: crypto.randomUUID(), product: p, count: firstCount(p, mode), ...(mode ? { mode } : {}), ...priced }]
      }
      // Повторный скан: ещё штука, канистра (в литрах — её объём) или килограмм.
      return ls.map((l, j) => (j === i ? { ...l, count: nextCount(p, l.mode, l.count) } : l))
    })
  }

  // «+ Оприходовать» у товара: экран открывается оприходованием с этим товаром в первой строке.
  const prefill = fix ? null : params.get('product')
  useEffect(() => {
    if (!prefill) return
    let alive = true
    void get<Product>(`/products/${prefill}`)
      .then((p) => {
        if (alive) addProduct(p)
      })
      .catch((e: unknown) => save.setError(e instanceof Error ? e.message : String(e)))
    return () => {
      alive = false
    }
    // Только при открытии экрана: товар из адреса добавляется один раз.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [prefill])

  /** Смена вида: строки остаются как есть — у масла свой режим «кан. | л» (SPEC-36 §13). */
  const switchKind = (k: ReceiptKind) => {
    if (k !== kind) setKind(k)
  }

  /** Этикетки на товар строки: столько же, сколько принимаем. */
  const printLine = (l: Line) => {
    const c = calcOf(l)
    const copies = typeof c === 'string' ? 1 : c.n
    // На этикетке — цена, которую товар получит этим приходом (проверка 20.10.2026).
    const product = typeof c === 'string' ? l.product : { ...l.product, sale_price_tyiyn: c.salePrice }
    void labels.run(() => printLabels([{ product, copies }]))
  }

  const updateLine = (key: string, patch: Partial<Line>) =>
    setLines((ls) => ls.map((l) => (l.key === key ? { ...l, ...patch } : l)))

  const calcs = lines.map(calcOf)
  let total: number | null = 0
  let totalSale: number | null = 0
  for (const c of calcs) {
    if (typeof c === 'string' || total === null || totalSale === null) {
      total = null
      totalSale = null
    } else {
      total += c.cost
      totalSale += c.sale
      if (!Number.isSafeInteger(total) || !Number.isSafeInteger(totalSale)) {
        total = null
        totalSale = null
      }
    }
  }

  const createSupplier = async () => {
    if (!newSupplier) return
    const name = newSupplier.name.trim()
    if (!name) {
      supplierAction.setError('Укажите название поставщика')
      return
    }
    const s = await supplierAction.run(() => post<Supplier>('/suppliers', { name, phone: newSupplier.phone.trim() }))
    if (s) {
      suppliers.setData((list) => [...(list ?? []), s])
      setSupplierId(s.id)
      setNewSupplier(null)
    }
  }

  // Чего не хватает для проведения: кнопка заблокирована, пока список не пуст.
  const badLine = calcs.findIndex((c) => typeof c === 'string')
  const part = partText.trim() ? parseSom(partText) : null
  const partError = payMode === 'part' ? partProblem(part, total) : null
  const paying = payMode !== 'debt'
  const now = paidNow(payMode, total, part)
  const missing = stockIn
    ? missingWithFocus(
        [lines.length > 0, 'товары — отсканируйте штрихкод', '[data-picker]'],
        [Boolean(comment.trim()), 'комментарий — откуда товар', '#receipt-comment'],
        [badLine < 0, `количество и цены в строке ${badLine + 1}`, `#line-${badLine + 1}-count`],
        [!fix || Boolean(fixComment.trim()), 'что и почему исправляете', '#correction-comment'],
      )
    : missingWithFocus(
        [lines.length > 0, 'товары — отсканируйте штрихкод', '[data-picker]'],
        [!onDebt || Boolean(supplierId), 'поставщика для накладной в долг', '#receipt-supplier'],
        [partError === null, partError ?? '', '#receipt-paid-now'],
        [!paying || Boolean(source), 'откуда оплачено', '#receipt-paid-from'],
        // Исправление: исходная оплата вернётся на счёт, остаток проверит сервер.
        [Boolean(fix) || !paying || !notEnough(sources, source, now), 'денег на выбранном счёте меньше оплаты', '#receipt-paid-from'],
        [badLine < 0, `количество и цены в строке ${badLine + 1}`, `#line-${badLine + 1}-count`],
        [unmatched.length === 0, 'строки накладной без товара — выберите товар или уберите строку', '#draft-unmatched'],
        [!fix || Boolean(fixComment.trim()), 'что и почему исправляете', '#correction-comment'],
      )
  // Итог накладной по фото против суммы строк прихода (SPEC-30 §9).
  const mismatch = draft && !stockIn ? totalMismatch(draft.total_tyiyn ?? draft.lines_sum_tyiyn, total) : null

  const addPhotos = (files: FileList) =>
    void photoAction.run(async () => {
      const ready = await preparePhotos(files)
      setPhotos((p) => [...p, ...ready])
    })

  const submit = () => {
    if (lines.length === 0) {
      save.setError('Добавьте хотя бы один товар')
      return
    }
    const body: { product_id: string; qty: number; cost_tyiyn: number; sale_price_tyiyn?: number }[] = []
    const copies: { product: Product; copies: number }[] = []
    for (let i = 0; i < lines.length; i++) {
      const c = calcs[i]
      if (typeof c === 'string') {
        save.setError(`Строка ${i + 1} (${lines[i].product.name}): ${c}`)
        return
      }
      // Новая цена продажи уходит, только если её поменяли: товар получит её при проведении.
      const changed = c.salePrice !== lines[i].product.sale_price_tyiyn
      body.push({ product_id: lines[i].product.id, qty: c.qty, cost_tyiyn: c.cost, ...(changed ? { sale_price_tyiyn: c.salePrice } : {}) })
      copies.push({ product: { ...lines[i].product, sale_price_tyiyn: c.salePrice }, copies: c.n })
    }
    if (total === null) {
      save.setError('Слишком большая сумма документа')
      return
    }
    if (stockIn) {
      submitStockIn(copies)
      return
    }
    void save.run(async () => {
      const doc = {
        supplier_id: supplierId || null,
        payment: onDebt ? 'debt' : 'paid',
        paid_from: paying ? source : null,
        ...(payMode === 'part' ? { paid_now_tyiyn: part } : {}),
        supplier_doc: supplierDoc.trim(),
        comment: comment.trim(),
        lines: body,
      }
      // Исправление: сторно исходной накладной и верная — одной операцией (SPEC-29 §24).
      // Накладная по фото: её фото прикрепится к приходу в той же операции (SPEC-30 §9).
      const r = fix
        ? { id: (await postCorrection({ op_id: opId, kind: 'receipt', original_id: fix.originalId, comment: fixComment.trim(), doc })).new_id }
        : await post<Receipt>('/receipts', { op_id: opId, ...doc, ...(draft ? { draft_id: draft.id } : {}) })
      // Фото — после проведения: накладная уже есть, не загрузившееся можно прикрепить в ней.
      if (photos.length) {
        const failed = await uploadPhotos(r.id, photos)
        if (failed.length) toast(`Приход проведён, но фото не загрузились — прикрепите в накладной: ${failed.join('; ')}`, 'error')
      }
      if (print) {
        try {
          await printLabels(copies)
        } catch (err) {
          console.error(err)
        }
      }
      navigate(`/receipts/${r.id}`)
    })
  }

  /** Оприходование: одна операция, один документ строками; цена — за штуку, канистру или кг. */
  const submitStockIn = (copies: { product: Product; copies: number }[]) => {
    const body: { product_id: string; qty: number; price_tyiyn: number; sale_price_tyiyn?: number }[] = []
    for (let i = 0; i < lines.length; i++) {
      const c = calcs[i]
      if (typeof c === 'string') return
      const changed = c.salePrice !== lines[i].product.sale_price_tyiyn
      // Цена — за штуку, канистру или кг: в режиме «л» строка переводит цену литра в цену канистры.
      body.push({ product_id: lines[i].product.id, qty: c.qty, price_tyiyn: c.unitPrice, ...(changed ? { sale_price_tyiyn: c.salePrice } : {}) })
    }
    void save.run(async () => {
      const doc = { comment: comment.trim(), lines: body }
      const id = fix
        ? (await postCorrection({ op_id: opId, kind: 'stock_in', original_id: fix.originalId, comment: fixComment.trim(), doc })).new_id
        : (await post<StockIn>('/stock-ins', { op_id: opId, ...doc })).id
      if (print) {
        try {
          await printLabels(copies)
        } catch (err) {
          console.error(err)
        }
      }
      navigate(`/stock-ins/${id}`)
    })
  }

  // Ctrl+Enter проводит документ, как в кассе (ui-rules §4); при открытом окне сочетание относится к окну.
  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      const dialog = Boolean(document.querySelector('[role="dialog"]'))
      if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && !dialog && !save.busy && missing.length === 0) {
        e.preventDefault()
        submit()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  })

  const activeSuppliers = (suppliers.data ?? []).filter((s) => s.active)
  const submitText = fix ? 'Сохранить исправление' : stockIn ? 'Провести оприходование' : 'Провести приход'

  return (
    <div className={lines.length > 0 ? 'pb-20 md:pb-0' : ''}>
      <PageHeader
        title={fix ? `Исправление ${stockIn ? 'оприходования' : 'прихода'}${fix.number ? ` № ${fix.number}` : ''}` : 'Новый приход'}
        actions={
          <Button variant="secondary" onClick={() => navigate(fix ? `/${stockIn ? 'stock-ins' : 'receipts'}/${fix.originalId}` : '/receipts')}>
            {fix ? 'Отменить' : 'К списку'}
          </Button>
        }
      />
      <div className="flex flex-col gap-4">
        {fix && (
          <>
            <CorrectionBanner label={stockIn ? 'оприходование' : 'приход'} number={fix.number} comment={fixComment} onComment={setFixComment} />
            {fix.state.rounded.length > 0 && (
              <div className="rounded-md bg-amber-50 px-3 py-2 text-sm text-amber-800">
                Цена за штуку округлена до тыйына — проверьте сумму строк: {fix.state.rounded.join(', ')}
              </div>
            )}
          </>
        )}
        {!fix && !draft && (
          <ReceiptDraftsBlock items={drafts.data?.items ?? []} onOpen={openDraft} onChanged={drafts.reload} />
        )}
        <ErrorBox error={draftAction.error ?? drafts.error} />
        {!fix && (
          <div className="flex flex-wrap gap-2" role="radiogroup" aria-label="Вид прихода">
            {(
              [
                ['receipt', 'От поставщика'],
                ['stock_in', 'Оприходование (без поставщика)'],
              ] as [ReceiptKind, string][]
            )
              .filter(([k]) => can(k === 'stock_in' ? 'stock_in' : 'receipts'))
              .map(([k, label]) => (
                <button
                  key={k}
                  type="button"
                  role="radio"
                  aria-checked={kind === k}
                  className={`rounded-md border px-4 py-2 text-sm font-medium ${kind === k ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white text-slate-700 hover:bg-slate-50'}`}
                  onClick={() => switchKind(k)}
                >
                  {label}
                </button>
              ))}
          </div>
        )}
        {stockIn ? (
          <Card>
            <p className="mb-3 text-sm text-slate-600">
              Товар пришёл без накладной и без денег: ляжет на склад по цене закупки, цена станет последней закупочной; владелец получит уведомление.
            </p>
            <Field label="Комментарий: откуда товар" required>
              <input id="receipt-comment" value={comment} onChange={(e) => setComment(e.target.value)} placeholder="Например: привёз владелец со склада" />
            </Field>
          </Card>
        ) : (
        <Card>
          {/* Ключа распознавания нет — владелец видит, как включить; администратор — ничего. */}
          {!fix && !draft && drafts.data && !drafts.data.enabled && owner && (
            <div className="mb-3 flex flex-wrap items-center gap-3 border-b border-slate-100 pb-3 text-sm text-slate-500" data-testid="photo-needs-key">
              <Button variant="secondary" disabled>
                Заполнить по фото
              </Button>
              <span>нужен ключ распознавания (Настройки → Телеграм и баллы / config.ps1)</span>
            </div>
          )}
          {!fix && (draft || drafts.data?.enabled) && (
            <div className="mb-3 flex flex-wrap items-start justify-between gap-3 border-b border-slate-100 pb-3">
              {draft ? (
                <div className="flex flex-wrap items-start gap-3" data-testid="draft-banner">
                  <DraftThumb id={draft.id} contentType={draft.content_type} size="h-20 w-20" />
                  <div className="flex flex-col gap-1 text-sm">
                    <div className="font-medium">Заполнено по фото накладной{draft.source === 'bot' ? ' из бота' : ''}</div>
                    {draft.message && <div className="text-amber-700">{draft.message}</div>}
                    {draft.supplier ? (
                      <div className="text-slate-600">
                        Поставщик найден по {draft.supplier.by === 'inn' ? 'ИНН' : 'названию'}: {draft.supplier.name}
                      </div>
                    ) : (
                      draft.invoice?.supplier_name && (
                        <div className="text-amber-700" data-testid="draft-supplier-hint">
                          Поставщик не найден. В накладной: {draft.invoice.supplier_name}
                          {draft.invoice.supplier_inn && `, ИНН ${draft.invoice.supplier_inn}`}{' '}
                          <button
                            type="button"
                            className="text-sky-700 underline"
                            onClick={() => setNewSupplier({ name: draft.invoice?.supplier_name ?? '', phone: '' })}
                          >
                            завести
                          </button>
                        </div>
                      )
                    )}
                    <div className="text-xs text-slate-500">
                      Распознанное — подсказка: проверьте строки и цены. Фото прикрепится к приходу при проведении.
                    </div>
                  </div>
                </div>
              ) : (
                <span className="text-sm text-slate-600">Накладная на бумаге? Сфотографируйте — приход заполнится сам.</span>
              )}
              {!draft && (
                <FillFromPhoto
                  onDraft={(d) => {
                    applyDraft(d)
                    setParams({ draft: d.id }, { replace: true })
                  }}
                />
              )}
            </div>
          )}
          <div className="grid grid-cols-1 gap-3 md:grid-cols-3">
            <Field label="Поставщик">
              <div className="flex gap-2">
                <select id="receipt-supplier" className="min-w-0 flex-1" value={supplierId} onChange={(e) => setSupplierId(e.target.value)}>
                  <option value="">— без поставщика —</option>
                  {activeSuppliers.map((s) => (
                    <option key={s.id} value={s.id}>
                      {s.name}
                    </option>
                  ))}
                </select>
                <Button variant="secondary" onClick={() => setNewSupplier(newSupplier ? null : { name: '', phone: '' })}>
                  + поставщик
                </Button>
              </div>
            </Field>
            <Field label="Номер документа поставщика">
              <input value={supplierDoc} onChange={(e) => setSupplierDoc(e.target.value)} />
            </Field>
            <Field label="Комментарий">
              <input id="receipt-comment" value={comment} onChange={(e) => setComment(e.target.value)} />
            </Field>
          </div>
          <ErrorBox error={suppliers.error} />
          {newSupplier && (
            <div className="mt-3 grid grid-cols-1 gap-3 rounded-md border border-slate-200 bg-slate-50 p-3 sm:grid-cols-[1fr_1fr_auto] sm:items-end">
              <Field label="Название">
                <input
                  autoFocus
                  value={newSupplier.name}
                  onChange={(e) => setNewSupplier({ ...newSupplier, name: e.target.value })}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      e.preventDefault()
                      void createSupplier()
                    }
                  }}
                />
              </Field>
              <Field label="Телефон">
                <input value={newSupplier.phone} onChange={(e) => setNewSupplier({ ...newSupplier, phone: e.target.value })} />
              </Field>
              <Button disabled={supplierAction.busy || !newSupplier.name.trim()} onClick={() => void createSupplier()}>
                Добавить
              </Button>
              <div className="sm:col-span-3">
                <ErrorBox error={supplierAction.error} />
              </div>
            </div>
          )}
        </Card>
        )}

        {unmatched.length > 0 && (
          <Card>
            <div id="draft-unmatched" tabIndex={-1} data-testid="draft-unmatched">
              <CardTitle>Строки накладной без товара: {unmatched.length}</CardTitle>
              <ul className="divide-y divide-slate-100">
                {unmatched.map((l) => (
                  <li key={l.no} className="flex flex-wrap items-center gap-2 py-2 text-sm" data-testid="draft-unmatched-line">
                    <div className="min-w-0 flex-1">
                      <div className="font-medium">
                        {l.no}. {l.name}
                      </div>
                      <div className="text-xs text-slate-500">
                        {[
                          l.code && `код ${l.code}`,
                          l.barcode && `штрихкод ${l.barcode}`,
                          `${l.quantity} ${l.unit}`.trim(),
                          l.price_tyiyn !== null && `по ${formatSom(l.price_tyiyn)}`,
                          l.sum_tyiyn !== null && `сумма ${formatSom(l.sum_tyiyn)}`,
                        ]
                          .filter(Boolean)
                          .join(' · ')}
                      </div>
                      <div className="text-xs text-rose-700">не найден — выберите товар или заведите новый</div>
                    </div>
                    <Button
                      variant={target === l.no ? 'primary' : 'secondary'}
                      onClick={() => {
                        setTarget(l.no)
                        focusId('receipt-picker')
                      }}
                    >
                      Выбрать товар
                    </Button>
                    <Button
                      variant="secondary"
                      onClick={() => {
                        setTarget(l.no)
                        setNewProductName(l.name)
                      }}
                    >
                      Новый товар
                    </Button>
                    <Button
                      variant="ghost"
                      aria-label={`Убрать строку накладной ${l.no}`}
                      title="Не принимать эту строку"
                      onClick={() => {
                        setUnmatched((us) => us.filter((x) => x.no !== l.no))
                        if (target === l.no) setTarget(null)
                      }}
                    >
                      ✕
                    </Button>
                  </li>
                ))}
              </ul>
            </div>
          </Card>
        )}

        <Card>
          {target !== null && (
            <div className="mb-2" data-testid="draft-target">
              <Notice tone="sky">
                Выберите товар для строки накладной «{unmatched.find((l) => l.no === target)?.name}» — найдите его в поиске{umagOn ? ', в том числе в старой базе Умаг' : ''}.{' '}
                <button type="button" className="underline" onClick={() => setTarget(null)}>
                  Отмена
                </button>
              </Notice>
            </div>
          )}
          <div className="mb-3">
            <ProductPicker
              inputId="receipt-picker"
              // Умаг виден в приходе от поставщика (SPEC-35 §2): строка, заведённая из Умага, берёт закупку
              // оттуда. В оприходовании — только заведённые товары, как на кассе.
              onPick={(p, how, umagDone) => addProduct(p, how, umagDone?.cost)}
              onUnknownCode={can('products') ? setUnknownCode : undefined}
              onCreate={can('products') ? setNewProductName : undefined}
              umag={!stockIn}
              umagAddLabel="Завести и добавить"
              placeholder={
                stockIn || !umagOn ? 'Сканируйте штрихкод или пишите название' : 'Сканируйте штрихкод или пишите название — подскажем и из старой базы Умаг'
              }
            />
          </div>
          {lines.length === 0 ? (
            <Empty>Сканируйте штрихкод или найдите товар по названию — он добавится строкой</Empty>
          ) : (
            <Table head={['Товар', 'Количество', stockIn ? 'Цена закупки' : 'Цена по накладной', 'Наценка', 'Цена продажи', 'Сумма', 'По продаже', '']}>
              {lines.map((l, i) => {
                const oil = isOil(l.product)
                const weight = l.product.unit === 'g'
                // Строка масла в литрах: цена — за литр, наценка — к закупке канистры.
                const tare = literTare(l.product, l.mode)
                const hints = literHints(l)
                const c = calcs[i]
                return (
                  <tr key={l.key} className="align-top">
                    <td className="px-2 py-2">
                      <div className="font-medium">
                        {i + 1}. {l.product.name}
                      </div>
                      <div className="text-xs text-slate-500">
                        {l.product.article && <span className="mr-2">{l.product.article}</span>}
                        остаток: {stockText(l.product)}
                      </div>
                      {l.hint && (
                        <div className="mt-0.5 max-w-xs text-xs text-slate-600" data-testid="draft-hint">
                          В накладной: «{l.hint.invoice}»{' '}
                          {l.hint.check && <Badge tone="amber">проверьте</Badge>}
                          {l.hint.note && <div className="text-amber-700">{l.hint.note}</div>}
                        </div>
                      )}
                    </td>
                    <td className="px-2 py-2">
                      {oil && (
                        // «кан. | л»: литрами — бочка, розлив, накладная в литрах (SPEC-36 §13).
                        <div className="mb-1 inline-flex overflow-hidden rounded border border-slate-300 text-xs" role="radiogroup" aria-label={`Единица строки ${i + 1}`}>
                          {(
                            [
                              ['can', 'кан.'],
                              ['l', 'л'],
                            ] as [QtyMode, string][]
                          ).map(([m, label]) => (
                            <button
                              key={m}
                              type="button"
                              role="radio"
                              aria-checked={l.mode === m}
                              data-testid={`line-${i + 1}-mode-${m}`}
                              className={`px-2 py-0.5 ${l.mode === m ? 'bg-sky-600 text-white' : 'bg-white text-slate-700 hover:bg-slate-50'}`}
                              onClick={() => setLines((ls) => ls.map((x) => (x.key === l.key ? switchMode(x, m) : x)))}
                            >
                              {label}
                            </button>
                          ))}
                        </div>
                      )}
                      <label className="flex flex-col gap-1 text-xs text-slate-500">
                        {weight
                          ? 'Кг'
                          : tare
                            ? 'Литров'
                            : oil
                              ? `Канистр (по ${formatLiters(l.product.container_ml ?? 0)})`
                              : 'Кол-во, шт'}
                        <input
                          id={`line-${i + 1}-count`}
                          className={`w-24 ${typeof c === 'string' ? 'border-rose-400' : ''}`}
                          inputMode={weight || tare ? 'decimal' : 'numeric'}
                          value={l.count}
                          onKeyDown={onFieldKey(i + 1, 'count')}
                          onChange={(e) => updateLine(l.key, { count: e.target.value })}
                        />
                      </label>
                      {hints?.split && (
                        <div className="mt-1 text-xs text-slate-500" data-testid="liters-split">
                          {hints.split}
                        </div>
                      )}
                    </td>
                    <td className="px-2 py-2">
                      <label className="flex flex-col gap-1 text-xs text-slate-500">
                        {tare ? 'Цена за литр, с' : oil ? 'Цена канистры, с' : weight ? 'Цена за кг, с' : 'Цена за шт, с'}
                        <input
                          id={`line-${i + 1}-price`}
                          className={`w-28 ${typeof c === 'string' ? 'border-rose-400' : ''}`}
                          inputMode="decimal"
                          value={l.price}
                          onKeyDown={onFieldKey(i + 1, 'price')}
                          onChange={(e) => updateLine(l.key, withCost(l, e.target.value, tare))}
                        />
                      </label>
                      {hints?.perCan && (
                        <div className="mt-1 text-xs text-slate-500" data-testid="liters-per-can">
                          {hints.perCan}
                        </div>
                      )}
                      {l.last && (
                        <div className="mt-1 text-xs text-slate-500" data-testid="last-receipt">
                          {lastText(l.last, lastPer(l.product))}
                        </div>
                      )}
                      {priceJumpText(l.price, l.last, tare) && (
                        <div className="mt-1 max-w-[14rem] rounded bg-amber-50 px-1.5 py-1 text-xs text-amber-800" data-testid="price-jump">
                          {priceJumpText(l.price, l.last, tare)}
                        </div>
                      )}
                    </td>
                    <td className="px-2 py-2">
                      <label className="flex flex-col gap-1 text-xs text-slate-500">
                        %
                        <input
                          id={`line-${i + 1}-markup`}
                          aria-label="Наценка, %"
                          className="w-20"
                          inputMode="decimal"
                          value={l.markup}
                          onKeyDown={onFieldKey(i + 1, 'markup')}
                          onChange={(e) => updateLine(l.key, withMarkup(l, e.target.value, tare))}
                        />
                      </label>
                    </td>
                    <td className="px-2 py-2">
                      <label className="flex flex-col gap-1 text-xs text-slate-500">
                        {oil ? 'Продажа канистры, с' : weight ? 'Продажа за кг, с' : 'Продажа за шт, с'}
                        <input
                          id={`line-${i + 1}-sale`}
                          aria-label="Цена продажи"
                          className={`w-28 ${typeof c !== 'string' && c.salePrice !== l.product.sale_price_tyiyn ? 'border-amber-500 bg-amber-50' : ''}`}
                          inputMode="decimal"
                          value={l.salePrice}
                          onKeyDown={onFieldKey(i + 1, 'sale')}
                          onChange={(e) => updateLine(l.key, withSalePrice(l, e.target.value, tare))}
                        />
                      </label>
                      {typeof c !== 'string' && c.salePrice !== l.product.sale_price_tyiyn && (
                        <div className="mt-1 text-xs text-amber-700" data-testid="new-sale-price">
                          было {formatSom(l.product.sale_price_tyiyn)} — новая цена с проведением
                        </div>
                      )}
                    </td>
                    <td className="whitespace-nowrap px-2 py-2 pt-7 text-right">
                      {typeof c === 'string' ? <span className="text-rose-600">{c}</span> : formatSom(c.cost)}
                    </td>
                    <td className="whitespace-nowrap px-2 py-2 pt-7 text-right text-slate-600">
                      {typeof c === 'string' ? '—' : formatSom(c.sale)}
                    </td>
                    <td className="px-2 py-2 pt-6 text-right">
                      <div className="flex flex-wrap justify-end gap-1">
                        <Button
                          variant="secondary"
                          className="px-2 py-1 text-xs"
                          disabled={labels.busy}
                          title="Напечатать этикетки на этот товар"
                          onClick={() => printLine(l)}
                        >
                          Этикетки
                        </Button>
                        <Button
                          variant="secondary"
                          className="px-2 py-1 text-xs"
                          title="Карточка товара: цена, штрихкоды, свой код"
                          onClick={() => setEditingProduct(l.product)}
                        >
                          Карточка
                        </Button>
                        <Button
                          variant="ghost"
                          className="px-2 py-1"
                          aria-label="Удалить строку"
                          onClick={() => setLines((ls) => ls.filter((x) => x.key !== l.key))}
                        >
                          ✕
                        </Button>
                      </div>
                    </td>
                  </tr>
                )
              })}
            </Table>
          )}
          <div className="mt-3 flex flex-col gap-3 border-t border-slate-200 pt-3">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div className="flex flex-wrap items-center gap-4">
                <Checkbox label="Печатать этикетки" checked={print} onChange={setPrint} />
                {!stockIn && (
                <div className="flex flex-wrap items-center gap-3 text-sm" role="radiogroup" aria-label="Расчёт с поставщиком">
                  {(
                    [
                      ['paid', 'Оплачена сразу'],
                      ['part', 'Часть сейчас, остаток в долг'],
                      ['debt', 'Вся в долг'],
                    ] as [PayMode, string][]
                  ).map(([m, label]) => (
                    <label key={m} className="flex items-center gap-1">
                      <input type="radio" name="receipt-pay" value={m} checked={payMode === m} onChange={() => setPayMode(m)} />
                      {label}
                    </label>
                  ))}
                </div>
                )}
                {!stockIn && payMode === 'part' && (
                  <label className="flex items-center gap-2 text-sm">
                    Оплачено сейчас, с
                    <input id="receipt-paid-now" className="w-28" inputMode="decimal" value={partText} onChange={(e) => setPartText(e.target.value)} />
                    {total !== null && part !== null && partError === null && (
                      <span className="text-slate-600" data-testid="receipt-debt-rest">
                        в долг {formatSom(total - part)}
                      </span>
                    )}
                  </label>
                )}
                {!stockIn && paying && (
                  <label className="flex items-center gap-2 text-sm">
                    {payMode === 'part' ? 'Со счёта' : 'Оплачено'}
                    <select id="receipt-paid-from" value={source} onChange={(e) => setPaidFrom(e.target.value)}>
                      {owner && <option value="">— откуда —</option>}
                      {sources.map((s) => (
                        <option key={s.value} value={s.value}>
                          {accountLabel({ name: s.label, balance_tyiyn: s.balance_tyiyn })}
                        </option>
                      ))}
                    </select>
                  </label>
                )}
              </div>
              <div className="flex flex-wrap items-center justify-end gap-4">
                <div className="text-right">
                  <div className="text-lg font-semibold">
                    Итого: <span className="tabular-nums" data-testid="receipt-total">{total === null ? '—' : formatSom(total)}</span>
                  </div>
                  <div className="text-sm text-slate-600">
                    По цене продажи: <span className="tabular-nums" data-testid="receipt-sale-total">{totalSale === null ? '—' : formatSom(totalSale)}</span>
                  </div>
                  {mismatch ? (
                    <div className="mt-1 rounded bg-amber-50 px-2 py-1 text-sm text-amber-800" data-testid="draft-total-mismatch">
                      {mismatch}
                    </div>
                  ) : (
                    draft &&
                    !stockIn &&
                    total !== null &&
                    (draft.total_tyiyn ?? draft.lines_sum_tyiyn) !== null && (
                      <div className="text-xs text-emerald-700" data-testid="draft-total-ok">
                        сходится с итогом накладной
                      </div>
                    )
                  )}
                </div>
                <div className="hidden md:block">
                  <Button disabled={save.busy || missing.length > 0} onClick={submit}>
                    {submitText}
                  </Button>
                </div>
              </div>
            </div>
            {!stockIn && (
            <div className="flex flex-col gap-2 border-t border-slate-100 pt-3">
              <div className="flex flex-wrap items-center gap-3 text-sm">
                <span className="font-medium">Фото накладной</span>
                <PhotoButtons busy={photoAction.busy} onPicked={addPhotos} />
                <span className="text-xs text-slate-500">прикрепятся при проведении; можно и позже — в накладной</span>
              </div>
              <PendingPhotos
                photos={photos}
                onRemove={(key) =>
                  setPhotos((list) => {
                    const gone = list.find((p) => p.key === key)
                    if (gone) URL.revokeObjectURL(gone.url)
                    return list.filter((p) => p.key !== key)
                  })
                }
              />
            </div>
            )}
            <ErrorBox error={save.error ?? labels.error ?? (stockIn ? null : accounts.error) ?? photoAction.error} />
            <Missing items={missing} className="md:self-end" />
          </div>
        </Card>
      </div>

      {lines.length > 0 && (
        <div className="no-print fixed inset-x-0 bottom-[52px] z-20 flex items-center gap-3 border-t border-slate-200 bg-white px-4 py-2 shadow-[0_-2px_8px_rgba(15,23,42,0.08)] md:hidden">
          <div className="min-w-0 flex-1">
            <div className="text-xs text-slate-500">
              {stockIn ? 'Итого · оприходование' : payMode === 'debt' ? 'Итого · в долг поставщику' : payMode === 'part' ? 'Итого · часть в долг' : 'Итого'}
            </div>
            <div className="truncate text-xl font-bold tabular-nums">{total === null ? '—' : formatSom(total)}</div>
          </div>
          <Button className="shrink-0 px-6 py-3 text-base" disabled={save.busy || missing.length > 0} onClick={submit}>
            {fix ? 'Сохранить' : stockIn ? 'Провести' : 'Провести приход'}
          </Button>
        </div>
      )}

      {unknownCode !== null && (
        <UnknownCodeModal
          code={unknownCode}
          onClose={() => setUnknownCode(null)}
          onLinked={(p) => {
            setUnknownCode(null)
            addProduct(p)
          }}
          onCreateNew={() => {
            setNewProductCode(unknownCode)
            setUnknownCode(null)
          }}
        />
      )}
      {editingProduct && (
        <ProductFormModal
          product={editingProduct}
          onClose={() => setEditingProduct(null)}
          onSaved={(p) => {
            setEditingProduct(null)
            // Цена и коды могли измениться — обновляем товар в строке.
            setLines((ls) => ls.map((l) => (l.product.id === p.id ? { ...l, product: p } : l)))
          }}
        />
      )}
      {newProductName !== null && (
        <ProductFormModal
          presetName={newProductName}
          onClose={() => setNewProductName(null)}
          onSaved={(p) => {
            setNewProductName(null)
            addProduct(p)
          }}
        />
      )}
      {newProductCode !== null && (
        <ProductFormModal
          presetBarcode={newProductCode}
          onClose={() => setNewProductCode(null)}
          onSaved={(p) => {
            setNewProductCode(null)
            addProduct(p)
          }}
        />
      )}
    </div>
  )
}
