import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { printLabels } from '../components/labels'
import { ProductFormModal } from '../components/ProductFormModal'
import { ProductPicker, stockText } from '../components/ProductPicker'
import { Button, Card, Checkbox, Empty, ErrorBox, Field, PageHeader, Table } from '../components/ui'
import { get, newOpId, post } from '../lib/api'
import { formatLiters, formatSom, parseSom } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import type { Product, Receipt, Supplier } from '../lib/types'

interface Line {
  key: string
  product: Product
  count: string
  price: string
}

const isOil = (p: Product): p is Product & { container_ml: number } => p.unit === 'ml' && Boolean(p.container_ml)

/** Целое положительное число из строки или null. */
function parseCount(s: string): number | null {
  const t = s.trim()
  if (!/^\d+$/.test(t)) return null
  const n = Number(t)
  return Number.isSafeInteger(n) && n > 0 ? n : null
}

/** Расчёт строки: количество в единицах учёта и сумма; текст ошибки, если ввод неверен. */
function lineCalc(l: Line): { n: number; qty: number; cost: number } | string {
  const n = parseCount(l.count)
  if (n === null) return isOil(l.product) ? 'укажите число канистр' : 'укажите количество'
  const price = parseSom(l.price)
  if (price === null) return 'укажите цену'
  const qty = isOil(l.product) ? n * l.product.container_ml : n
  const cost = n * price
  if (!Number.isSafeInteger(qty) || !Number.isSafeInteger(cost)) return 'слишком большое число'
  return { n, qty, cost }
}

export default function ReceiptNew() {
  const navigate = useNavigate()
  const [opId] = useState(newOpId)
  const suppliers = useLoad(() => get<Supplier[]>('/suppliers'), [])
  const [supplierId, setSupplierId] = useState('')
  const [supplierDoc, setSupplierDoc] = useState('')
  const [comment, setComment] = useState('')
  const [lines, setLines] = useState<Line[]>([])
  const [print, setPrint] = useState(false)
  const [unknownCode, setUnknownCode] = useState<string | null>(null)
  const [newSupplier, setNewSupplier] = useState<{ name: string; phone: string } | null>(null)
  const save = useAction()
  const supplierAction = useAction()

  const addProduct = (p: Product) => {
    setLines((ls) => {
      const i = ls.findIndex((l) => l.product.id === p.id)
      if (i < 0) return [...ls, { key: crypto.randomUUID(), product: p, count: '1', price: '' }]
      return ls.map((l, j) => {
        if (j !== i) return l
        const n = parseCount(l.count)
        return { ...l, count: String(n === null ? 1 : n + 1) }
      })
    })
  }

  const updateLine = (key: string, patch: Partial<Line>) =>
    setLines((ls) => ls.map((l) => (l.key === key ? { ...l, ...patch } : l)))

  const calcs = lines.map(lineCalc)
  let total: number | null = 0
  for (const c of calcs) {
    if (typeof c === 'string' || total === null) {
      total = null
    } else {
      total += c.cost
      if (!Number.isSafeInteger(total)) total = null
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

  const submit = () => {
    if (lines.length === 0) {
      save.setError('Добавьте хотя бы один товар')
      return
    }
    const body: { product_id: string; qty: number; cost_tyiyn: number }[] = []
    const copies: { product: Product; copies: number }[] = []
    for (let i = 0; i < lines.length; i++) {
      const c = calcs[i]
      if (typeof c === 'string') {
        save.setError(`Строка ${i + 1} (${lines[i].product.name}): ${c}`)
        return
      }
      body.push({ product_id: lines[i].product.id, qty: c.qty, cost_tyiyn: c.cost })
      copies.push({ product: lines[i].product, copies: c.n })
    }
    if (total === null) {
      save.setError('Слишком большая сумма документа')
      return
    }
    void save.run(async () => {
      const r = await post<Receipt>('/receipts', {
        op_id: opId,
        supplier_id: supplierId || null,
        supplier_doc: supplierDoc.trim(),
        comment: comment.trim(),
        lines: body,
      })
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

  const activeSuppliers = (suppliers.data ?? []).filter((s) => s.active)

  return (
    <div>
      <PageHeader
        title="Новый приход"
        actions={
          <Button variant="secondary" onClick={() => navigate('/receipts')}>
            К списку
          </Button>
        }
      />
      <div className="flex flex-col gap-4">
        <Card>
          <div className="grid grid-cols-1 gap-3 md:grid-cols-3">
            <Field label="Поставщик">
              <div className="flex gap-2">
                <select className="min-w-0 flex-1" value={supplierId} onChange={(e) => setSupplierId(e.target.value)}>
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
              <input value={comment} onChange={(e) => setComment(e.target.value)} />
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
              <Button disabled={supplierAction.busy} onClick={() => void createSupplier()}>
                Добавить
              </Button>
              <div className="sm:col-span-3">
                <ErrorBox error={supplierAction.error} />
              </div>
            </div>
          )}
        </Card>

        <Card>
          <div className="mb-3">
            <ProductPicker onPick={addProduct} onUnknownCode={setUnknownCode} />
          </div>
          {lines.length === 0 ? (
            <Empty>Сканируйте товары или найдите их по названию</Empty>
          ) : (
            <Table head={['Товар', 'Количество', 'Цена, с', 'Сумма', '']}>
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
                        {oil ? `Канистр (по ${formatLiters(l.product.container_ml ?? 0)})` : 'Кол-во, шт'}
                        <input
                          className="w-24"
                          inputMode="numeric"
                          value={l.count}
                          onChange={(e) => updateLine(l.key, { count: e.target.value })}
                        />
                      </label>
                    </td>
                    <td className="px-2 py-2">
                      <label className="flex flex-col gap-1 text-xs text-slate-500">
                        {oil ? 'Цена канистры, с' : 'Цена за шт, с'}
                        <input
                          className="w-32"
                          inputMode="decimal"
                          value={l.price}
                          onChange={(e) => updateLine(l.key, { price: e.target.value })}
                        />
                      </label>
                    </td>
                    <td className="whitespace-nowrap px-2 py-2 pt-7 text-right">
                      {typeof c === 'string' ? <span className="text-rose-600">{c}</span> : formatSom(c.cost)}
                    </td>
                    <td className="px-2 py-2 pt-6 text-right">
                      <Button
                        variant="ghost"
                        aria-label="Удалить строку"
                        onClick={() => setLines((ls) => ls.filter((x) => x.key !== l.key))}
                      >
                        ✕
                      </Button>
                    </td>
                  </tr>
                )
              })}
            </Table>
          )}
          <div className="mt-3 flex flex-wrap items-center justify-between gap-3 border-t border-slate-200 pt-3">
            <Checkbox label="Печатать этикетки" checked={print} onChange={setPrint} />
            <div className="text-lg font-semibold">Итого: {total === null ? '—' : formatSom(total)}</div>
          </div>
        </Card>

        <ErrorBox error={save.error} />
        <div className="flex justify-end">
          <Button disabled={save.busy || lines.length === 0} onClick={submit}>
            Провести приход
          </Button>
        </div>
      </div>

      {unknownCode !== null && (
        <ProductFormModal
          presetBarcode={unknownCode}
          onClose={() => setUnknownCode(null)}
          onSaved={(p) => {
            setUnknownCode(null)
            addProduct(p)
          }}
        />
      )}
    </div>
  )
}
