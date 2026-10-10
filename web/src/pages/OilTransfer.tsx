// Перелив масла: остаток из одного масла в другое, себестоимость получателя — средневзвешенная (SPEC-14).
import { useState } from 'react'
import { Button, Card, Empty, ErrorBox, Field, Loading, Missing, Modal, PageHeader, Table, toast } from '../components/ui'
import { get, newOpId, patch, post } from '../lib/api'
import { formatDateTime, formatLiters, formatSom, parseLiters, parseSom, somInput } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import { divRound } from '../lib/money'
import type { Product } from '../lib/types'

interface OilState {
  product_id: string
  name: string
  stock_ml: number
  value_tyiyn: number
  avg_per_l_tyiyn: number | null
}

interface TransferOut {
  id: string
  number: number
  qty_ml: number
  value_tyiyn: number
  from: OilState
  to: OilState
}

interface TransferRow {
  id: string
  number: number
  from_name: string
  to_name: string
  qty_ml: number
  value_tyiyn: number
  comment: string
  user_name: string
  created_at: string
  reversed: boolean
}

/** Себестоимость литра для показа: стоимость остатка × 1000 / мл. */
const perLiter = (value: number, ml: number): number | null => (ml > 0 ? divRound(value * 1000, ml) : null)

/** Миллилитры для поля ввода литров: 12500 → «12,5». */
function litersInput(ml: number): string {
  const frac = String(ml % 1000).padStart(3, '0').replace(/0+$/, '')
  return `${Math.trunc(ml / 1000)}${frac ? ',' + frac : ''}`
}

export default function OilTransfer() {
  const products = useLoad(() => get<Product[]>('/products?limit=500'), [])
  const history = useLoad(() => get<TransferRow[]>('/oil/transfers'), [])
  const [form, setForm] = useState({ from: '', to: '', liters: '', comment: '' })
  const [opId, setOpId] = useState(newOpId)
  const [done, setDone] = useState<TransferOut | null>(null)
  const [prices, setPrices] = useState({ canister: '', pour: '' })
  const act = useAction()
  const priceAct = useAction()
  const [reversing, setReversing] = useState<{ row: TransferRow; comment: string; opId: string } | null>(null)
  const revAct = useAction()

  const oils = (products.data ?? []).filter((p) => p.unit === 'ml' && !p.archived)
  const from = oils.find((p) => p.id === form.from)
  const to = oils.find((p) => p.id === form.to)
  const ml = form.liters.trim() ? parseLiters(form.liters) : null

  // Предпросмотр: стоимость уходит по средней источника, у получателя — средневзвешенная.
  const preview = (() => {
    if (!from || !to || ml === null || ml <= 0 || from.stock_value_tyiyn === undefined || to.stock_value_tyiyn === undefined) return null
    const moved = from.stock_qty > 0 ? (ml >= from.stock_qty ? from.stock_value_tyiyn : divRound(from.stock_value_tyiyn * ml, from.stock_qty)) : 0
    return {
      before: perLiter(to.stock_value_tyiyn, to.stock_qty),
      after: perLiter(to.stock_value_tyiyn + moved, to.stock_qty + ml),
      source: perLiter(from.stock_value_tyiyn, from.stock_qty),
      totalMl: to.stock_qty + ml,
    }
  })()

  const notFilled = missingWithFocus(
    [Boolean(form.from), 'из какого масла', '#oil-from'],
    [Boolean(form.to), 'в какое масло', '#oil-to'],
    [!form.from || form.from !== form.to, 'разные масла', '#oil-to'],
    [ml !== null && ml > 0, 'сколько литров', '#oil-liters'],
    [!from || ml === null || ml <= from.stock_qty, `не больше остатка: ${from ? formatLiters(Math.max(from.stock_qty, 0)) : ''}`, '#oil-liters'],
  )

  const submit = () =>
    void act.run(async () => {
      if (ml === null) return
      const out = await post<TransferOut>('/oil/transfers', {
        op_id: opId,
        from_product_id: form.from,
        to_product_id: form.to,
        qty_ml: ml,
        comment: form.comment,
      })
      setDone(out)
      setOpId(newOpId())
      setPrices({ canister: somInput(to?.sale_price_tyiyn ?? 0), pour: to?.pour_price_per_l_tyiyn ? somInput(to.pour_price_per_l_tyiyn) : '' })
      setForm({ from: '', to: '', liters: '', comment: '' })
      products.reload()
      history.reload()
      toast(`Перелито ${formatLiters(out.qty_ml)}`)
    })

  const savePrices = () =>
    void priceAct.run(async () => {
      if (!done) return
      const canister = parseSom(prices.canister)
      const pour = prices.pour.trim() ? parseSom(prices.pour) : null
      if (canister === null || (prices.pour.trim() && pour === null)) throw new Error('Неверная цена')
      await patch(`/products/${done.to.product_id}/prices`, { sale_price_tyiyn: canister, pour_price_per_l_tyiyn: pour })
      setDone(null)
      products.reload()
      toast('Цены обновлены')
    })

  const doneProduct = done ? oils.find((p) => p.id === done.to.product_id) : undefined

  return (
    <div className="flex flex-col gap-4">
      <PageHeader title="Перелив масла" />
      <ErrorBox error={products.error} />

      <Card className="flex flex-col gap-3">
        <details className="text-sm text-slate-600">
          <summary className="cursor-pointer select-none text-sky-700 hover:text-sky-800">Как считается?</summary>
          <p className="mt-1">
            Остаток переходит из одного масла в другое вместе со стоимостью. Себестоимость литра у получателя становится средней: 5 л по 100 с и 50 л
            по 200 с дают 190,91 с за литр.
          </p>
        </details>
        {products.loading && !products.data ? (
          <Loading />
        ) : oils.length < 2 ? (
          <Empty>Для перелива нужно хотя бы два масла в справочнике товаров.</Empty>
        ) : (
          <>
            <div className="grid gap-3 sm:grid-cols-2">
              <Field label="Из масла" required>
                <select
                  id="oil-from"
                  value={form.from}
                  onChange={(e) => {
                    const p = oils.find((o) => o.id === e.target.value)
                    setForm({ ...form, from: e.target.value, liters: p && p.stock_qty > 0 ? litersInput(p.stock_qty) : '' })
                  }}
                >
                  <option value="">— выберите —</option>
                  {oils
                    .filter((p) => p.stock_qty > 0)
                    .map((p) => (
                      <option key={p.id} value={p.id}>
                        {p.name} · {formatLiters(p.stock_qty)}
                      </option>
                    ))}
                </select>
              </Field>
              <Field label="В масло" required>
                <select id="oil-to" value={form.to} onChange={(e) => setForm({ ...form, to: e.target.value })}>
                  <option value="">— выберите —</option>
                  {oils
                    .filter((p) => p.id !== form.from)
                    .map((p) => (
                      <option key={p.id} value={p.id}>
                        {p.name} · {formatLiters(Math.max(p.stock_qty, 0))}
                      </option>
                    ))}
                </select>
              </Field>
            </div>
            <div className="grid gap-3 sm:grid-cols-2">
              <Field label="Сколько, л" required hint="По умолчанию — весь остаток источника">
                <input id="oil-liters" inputMode="decimal" value={form.liters} onChange={(e) => setForm({ ...form, liters: e.target.value })} />
              </Field>
              <Field label="Комментарий">
                <input value={form.comment} onChange={(e) => setForm({ ...form, comment: e.target.value })} placeholder="например, освободили бочку" />
              </Field>
            </div>
            {preview && from && to && (
              <div className="grid gap-2 rounded-md bg-slate-50 p-3 text-sm sm:grid-cols-3">
                <div>
                  <div className="text-xs text-slate-500">Из «{from.name}»</div>
                  <div>{preview.source === null ? '—' : `${formatSom(preview.source)} за л`}</div>
                </div>
                <div>
                  <div className="text-xs text-slate-500">В «{to.name}» сейчас</div>
                  <div>
                    {formatLiters(Math.max(to.stock_qty, 0))}
                    {preview.before !== null && ` по ${formatSom(preview.before)} за л`}
                  </div>
                </div>
                <div>
                  <div className="text-xs text-slate-500">Станет</div>
                  <div className="font-semibold">
                    {formatLiters(preview.totalMl)}
                    {preview.after !== null && ` по ${formatSom(preview.after)} за л`}
                  </div>
                </div>
              </div>
            )}
            <Missing items={notFilled} />
            <ErrorBox error={act.error} />
            <div className="flex justify-end">
              <Button disabled={act.busy || notFilled.length > 0} onClick={submit}>
                Перелить
              </Button>
            </div>
          </>
        )}
      </Card>

      {done && (
        <Card className="flex flex-col gap-3 border-emerald-300 bg-emerald-50">
          <div>
            <div className="font-medium">
              Перелив № {done.number}: {formatLiters(done.qty_ml)} из «{done.from.name}» в «{done.to.name}»
            </div>
            <div className="text-sm text-slate-700">
              В «{done.to.name}» теперь {formatLiters(done.to.stock_ml)}
              {done.to.avg_per_l_tyiyn !== null && `, себестоимость ${formatSom(done.to.avg_per_l_tyiyn)} за литр`}.
              {doneProduct?.container_ml && done.to.avg_per_l_tyiyn !== null &&
                ` Канистра ${formatLiters(doneProduct.container_ml)} обходится в ${formatSom(divRound(done.to.avg_per_l_tyiyn * doneProduct.container_ml, 1000))}.`}
            </div>
          </div>
          <div className="text-sm font-medium">Цены продажи «{done.to.name}»</div>
          <div className="grid gap-3 sm:grid-cols-[1fr_1fr_auto] sm:items-end">
            <Field label="Канистра, с">
              <input inputMode="decimal" value={prices.canister} onChange={(e) => setPrices({ ...prices, canister: e.target.value })} />
            </Field>
            <Field label="Розлив, с за литр" hint="Пусто — не продаётся на розлив">
              <input inputMode="decimal" value={prices.pour} onChange={(e) => setPrices({ ...prices, pour: e.target.value })} />
            </Field>
            <div className="flex gap-2">
              <Button variant="secondary" onClick={() => setDone(null)}>
                Оставить
              </Button>
              <Button disabled={priceAct.busy} onClick={savePrices}>
                Сохранить цены
              </Button>
            </div>
          </div>
          <ErrorBox error={priceAct.error} />
        </Card>
      )}

      <Card>
        <h2 className="mb-3 font-semibold">История переливов</h2>
        <ErrorBox error={history.error} />
        {(history.data ?? []).length === 0 ? (
          <Empty>Переливов ещё не было</Empty>
        ) : (
          <Table head={['№', 'Когда', 'Откуда', 'Куда', 'Сколько', 'Стоимость', 'Кто', '']}>
            {(history.data ?? []).map((t) => (
              <tr key={t.id} className={t.reversed ? 'text-slate-400 line-through' : ''}>
                <td className="px-2 py-2 font-medium">{t.number}</td>
                <td className="whitespace-nowrap px-2 py-2">{formatDateTime(t.created_at)}</td>
                <td className="px-2 py-2">{t.from_name}</td>
                <td className="px-2 py-2">{t.to_name}</td>
                <td className="whitespace-nowrap px-2 py-2">{formatLiters(t.qty_ml)}</td>
                <td className="whitespace-nowrap px-2 py-2">{formatSom(t.value_tyiyn)}</td>
                <td className="px-2 py-2">{t.user_name}</td>
                <td className="px-2 py-2 text-right">
                  {t.reversed ? (
                    <span className="no-underline">отменён</span>
                  ) : (
                    <Button variant="secondary" className="px-2 py-1 text-xs" onClick={() => setReversing({ row: t, comment: '', opId: newOpId() })}>
                      Отменить
                    </Button>
                  )}
                </td>
              </tr>
            ))}
          </Table>
        )}
      </Card>
      {reversing && (
        <Modal title={`Отменить перелив № ${reversing.row.number}`} onClose={() => setReversing(null)}>
          <div className="flex flex-col gap-3">
            <div className="text-sm text-slate-600">
              {formatLiters(reversing.row.qty_ml)} вернутся из «{reversing.row.to_name}» в «{reversing.row.from_name}» той же стоимостью{' '}
              {formatSom(reversing.row.value_tyiyn)}. Если перелитое уже продано, отменить нельзя — тогда перелейте обратно.
            </div>
            <Field label="Причина" required>
              <input autoFocus value={reversing.comment} onChange={(e) => setReversing({ ...reversing, comment: e.target.value })} />
            </Field>
            <ErrorBox error={revAct.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setReversing(null)}>
                Не отменять
              </Button>
              <Button
                disabled={revAct.busy || !reversing.comment.trim()}
                onClick={() =>
                  void revAct.run(async () => {
                    await post(`/oil/transfers/${reversing.row.id}/reverse`, { op_id: reversing.opId, comment: reversing.comment })
                    setReversing(null)
                    products.reload()
                    history.reload()
                    toast('Перелив отменён')
                  })
                }
              >
                Отменить перелив
              </Button>
            </div>
          </div>
        </Modal>
      )}
    </div>
  )
}
