// Новый приход: сканер, создание незнакомых товаров, цены из прошлого прихода, печать этикеток (SPEC-03).
// Строка как в старой программе: цена по накладной, наценка %, цена продажи, сумма; наценка и цена
// продажи связаны, новая цена продажи ставится товару при проведении (SPEC-28 §1, ADR-075). Внизу
// итог по закупке и по продаже; при выборе товара — «в прошлый раз: цена, дата, поставщик» (SPEC-27).
// Ввод подряд с клавиатуры и подсказки из старой программы, часть оплаты сразу, фото накладной (SPEC-29).
// Вверху — вид документа: «От поставщика» или «Оприходование (без поставщика)» (SPEC-29 §8, ADR-086):
// те же строки, но без поставщика и оплаты, с обязательным комментарием; количество — как на складе.
import { useEffect, useState, type KeyboardEvent } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { CorrectionBanner } from '../components/Correction'
import { printLabels } from '../components/labels'
import { ProductFormModal } from '../components/ProductFormModal'
import { UnknownCodeModal } from '../components/UnknownCodeModal'
import { ProductPicker, stockText, type PickHow } from '../components/ProductPicker'
import { PendingPhotos, PhotoButtons } from '../components/ReceiptPhotos'
import { preparePhotos, uploadPhotos, type PendingPhoto } from '../lib/photoUpload'
import { Button, Card, Checkbox, Empty, ErrorBox, Field, Missing, PageHeader, Table, toast } from '../components/ui'
import { accountLabel } from '../lib/accounts'
import { get, newOpId, post } from '../lib/api'
import { postCorrection } from '../lib/corrections'
import type { ReceiptFixState } from '../lib/receiptFix'
import { formatLiters, formatSom, parseSom } from '../lib/format'
import { initialPrices, isOil, lastText, lineCalc, parseCount, withCost, withMarkup, withSalePrice } from '../lib/receiptLines'
import { missingWithFocus } from '../lib/forms'
import { useFocusAfterRender } from '../lib/focus'
import { useUser } from '../lib/auth'
import { useAction, useLoad } from '../lib/hooks'
import { presetFromHit, type CardPreset } from '../lib/oldCatalog'
import { nextField, type LineField } from '../lib/receiptKeys'
import { defaultPaySource, notEnough, paidNow, partProblem, paySources, type PayMode } from '../lib/receiptPay'
import { convertCount, stockInFirstCount, stockInLineCalc, stockInNextCount } from '../lib/stockIn'
import type { CashAccount, Category, LastReceipt, OldCatalogHit, Product, Receipt, StockIn, Supplier } from '../lib/types'

/** Вид документа на экране: приход от поставщика или оприходование без поставщика. */
export type ReceiptKind = 'receipt' | 'stock_in'

interface Line {
  key: string
  product: Product
  count: string
  /** Цена по накладной за штуку или канистру. */
  price: string
  /** Наценка над ценой по накладной, %, с одним знаком. */
  markup: string
  /** Цена продажи за штуку или канистру; по умолчанию — текущая цена товара. */
  salePrice: string
  /** Прошлая закупка: undefined — ещё грузится, null — закупок не было. */
  last?: LastReceipt | null
}

/** Исправление прихода владельцем (SPEC-29 §24): тот же экран, заполненный исходной накладной. */
export interface ReceiptFix {
  /** Что исправляют: накладную или оприходование; по умолчанию — накладную. */
  kind?: ReceiptKind
  originalId: string
  number: number | null
  state: ReceiptFixState
}

export default function ReceiptNew({ fix }: { fix?: ReceiptFix }) {
  const navigate = useNavigate()
  const [params] = useSearchParams()
  const [opId] = useState(newOpId)
  const [kind, setKind] = useState<ReceiptKind>(fix?.kind ?? (params.get('kind') === 'stock_in' ? 'stock_in' : 'receipt'))
  const stockIn = kind === 'stock_in'
  const calcOf = stockIn ? stockInLineCalc : lineCalc
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
  const categories = useLoad(() => get<Category[]>('/categories'), [])
  const [oldPreset, setOldPreset] = useState<CardPreset | null>(null)
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

  const addProduct = (p: Product, how: PickHow = 'search', cost?: string) => {
    // Скан — фокус остаётся в поиске (сканируют подряд); выбор из списка — сразу к количеству строки.
    const at = lines.findIndex((l) => l.product.id === p.id)
    if (how === 'search') focusId(`line-${at < 0 ? lines.length + 1 : at + 1}-count`)
    // Прошлая закупка: цена, дата и поставщик — подсказка рядом с ценой (SPEC-27).
    if (!lines.some((l) => l.product.id === p.id)) {
      void get<LastReceipt | null>(`/products/${p.id}/last-receipt`)
        .then((last) => setLines((ls) => ls.map((l) => (l.product.id === p.id ? { ...l, last } : l))))
        .catch(() => setLines((ls) => ls.map((l) => (l.product.id === p.id ? { ...l, last: null } : l))))
    }
    setLines((ls) => {
      const i = ls.findIndex((l) => l.product.id === p.id)
      if (i < 0) {
        // Цена подставляется из прошлого прихода, цена продажи — текущая; их можно изменить.
        // Карточка из старой программы — закупка оттуда, пока своих приходов нет.
        const prices = initialPrices(p)
        const count = stockIn ? stockInFirstCount(p) : '1'
        return [...ls, { key: crypto.randomUUID(), product: p, count, ...(cost && !prices.price ? withCost(prices, cost) : prices) }]
      }
      return ls.map((l, j) => {
        if (j !== i) return l
        if (stockIn) return { ...l, count: stockInNextCount(p, l.count) }
        const n = parseCount(l.count)
        return { ...l, count: String(n === null ? 1 : n + 1) }
      })
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

  /** Смена вида: строки остаются, количество масла — канистрами в приходе, литрами в оприходовании. */
  const switchKind = (k: ReceiptKind) => {
    if (k === kind) return
    setKind(k)
    setLines((ls) => ls.map((l) => ({ ...l, count: convertCount(l.product, l.count, k === 'stock_in') })))
  }

  /** Этикетки на товар строки: столько же, сколько принимаем. */
  const printLine = (l: Line) => {
    const c = calcOf(l)
    const copies = typeof c === 'string' ? 1 : c.n
    void labels.run(() => printLabels([{ product: l.product, copies }]))
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
        [!fix || Boolean(fixComment.trim()), 'что и почему исправляете', '#correction-comment'],
      )

  const addPhotos = (files: FileList) =>
    void photoAction.run(async () => {
      const ready = await preparePhotos(files)
      setPhotos((p) => [...p, ...ready])
    })

  /** Позиция старой программы: карточка с её данными, проверить и сохранить (SPEC-29 §1–2). */
  const fromOld = (h: OldCatalogHit, code: string) => setOldPreset(presetFromHit(h, categories.data ?? [], '', code))

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
      copies.push({ product: lines[i].product, copies: c.n })
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
      const r = fix
        ? { id: (await postCorrection({ op_id: opId, kind: 'receipt', original_id: fix.originalId, comment: fixComment.trim(), doc })).new_id }
        : await post<Receipt>('/receipts', { op_id: opId, ...doc })
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
      const price = parseSom(lines[i].price)
      if (typeof c === 'string' || price === null) return
      const changed = c.salePrice !== lines[i].product.sale_price_tyiyn
      body.push({ product_id: lines[i].product.id, qty: c.qty, price_tyiyn: price, ...(changed ? { sale_price_tyiyn: c.salePrice } : {}) })
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
        {!fix && (
          <div className="flex flex-wrap gap-2" role="radiogroup" aria-label="Вид прихода">
            {(
              [
                ['receipt', 'От поставщика'],
                ['stock_in', 'Оприходование (без поставщика)'],
              ] as [ReceiptKind, string][]
            ).map(([k, label]) => (
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

        <Card>
          <div className="mb-3">
            <ProductPicker
              inputId="receipt-picker"
              onPick={(p, how) => addProduct(p, how)}
              onUnknownCode={setUnknownCode}
              onCreate={setNewProductName}
              onPickOld={fromOld}
              placeholder="Сканируйте штрихкод или пишите название — подскажем и из старой программы"
            />
          </div>
          {lines.length === 0 ? (
            <Empty>Сканируйте штрихкод или найдите товар по названию — он добавится строкой</Empty>
          ) : (
            <Table head={['Товар', 'Количество', stockIn ? 'Цена закупки' : 'Цена по накладной', 'Наценка', 'Цена продажи', 'Сумма', 'По продаже', '']}>
              {lines.map((l, i) => {
                const oil = isOil(l.product)
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
                    </td>
                    <td className="px-2 py-2">
                      <label className="flex flex-col gap-1 text-xs text-slate-500">
                        {stockIn && l.product.unit === 'ml'
                          ? 'Литров'
                          : stockIn && l.product.unit === 'g'
                            ? 'Кг'
                            : oil
                              ? `Канистр (по ${formatLiters(l.product.container_ml ?? 0)})`
                              : 'Кол-во, шт'}
                        <input
                          id={`line-${i + 1}-count`}
                          className={`w-24 ${typeof c === 'string' ? 'border-rose-400' : ''}`}
                          inputMode={stockIn && l.product.unit !== 'piece' ? 'decimal' : 'numeric'}
                          value={l.count}
                          onKeyDown={onFieldKey(i + 1, 'count')}
                          onChange={(e) => updateLine(l.key, { count: e.target.value })}
                        />
                      </label>
                    </td>
                    <td className="px-2 py-2">
                      <label className="flex flex-col gap-1 text-xs text-slate-500">
                        {oil ? 'Цена канистры, с' : stockIn && l.product.unit === 'g' ? 'Цена за кг, с' : 'Цена за шт, с'}
                        <input
                          id={`line-${i + 1}-price`}
                          className={`w-28 ${typeof c === 'string' ? 'border-rose-400' : ''}`}
                          inputMode="decimal"
                          value={l.price}
                          onKeyDown={onFieldKey(i + 1, 'price')}
                          onChange={(e) => updateLine(l.key, withCost(l, e.target.value))}
                        />
                      </label>
                      {l.last && (
                        <div className="mt-1 text-xs text-slate-500" data-testid="last-receipt">
                          {lastText(l.last)}
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
                          onChange={(e) => updateLine(l.key, withMarkup(l, e.target.value))}
                        />
                      </label>
                    </td>
                    <td className="px-2 py-2">
                      <label className="flex flex-col gap-1 text-xs text-slate-500">
                        {oil ? 'Продажа канистры, с' : stockIn && l.product.unit === 'g' ? 'Продажа за кг, с' : 'Продажа за шт, с'}
                        <input
                          id={`line-${i + 1}-sale`}
                          aria-label="Цена продажи"
                          className={`w-28 ${typeof c !== 'string' && c.salePrice !== l.product.sale_price_tyiyn ? 'border-amber-500 bg-amber-50' : ''}`}
                          inputMode="decimal"
                          value={l.salePrice}
                          onKeyDown={onFieldKey(i + 1, 'sale')}
                          onChange={(e) => updateLine(l.key, withSalePrice(l, e.target.value))}
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
      {oldPreset && (
        <ProductFormModal
          preset={oldPreset}
          onClose={() => setOldPreset(null)}
          onSaved={(p) => {
            const cost = oldPreset.cost
            setOldPreset(null)
            addProduct(p, 'search', cost)
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
