import { useState } from 'react'
import { ProductPicker } from '../components/ProductPicker'
import { DirSwitch } from '../components/DirSwitch'
import { Button, Card, CardTitle, Empty, ErrorBox, Field, Loading, Missing, Money, PageHeader, Table } from '../components/ui'
import { get, post, patch, qs } from '../lib/api'
import { formatLiters, formatSom, parseLiters, todayBishkek } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import type { GiftRule, Product } from '../lib/types'

interface GiftReportRow {
  product_id: string
  name: string
  unit: 'piece' | 'ml'
  qty: number
  checks: number
  cost_tyiyn: number
}

/** Что и сколько подарили за период и во что это обошлось по себестоимости (SPEC-11). */
function GiftReport() {
  const [from, setFrom] = useState(todayBishkek().slice(0, 8) + '01')
  const [to, setTo] = useState(todayBishkek())
  const report = useLoad(() => get<GiftReportRow[]>(`/reports/gifts${qs({ from, to })}`), [from, to])
  const rows = report.data ?? []
  const total = rows.reduce((acc, r) => acc + r.cost_tyiyn, 0)
  return (
    <Card className="mt-4 flex flex-col gap-3 [&_th:nth-child(4)]:text-right">
      <div className="flex flex-wrap items-end gap-3">
        <h2 className="mr-auto text-base font-semibold">Подарено за период</h2>
        <Field label="С">
          <input type="date" value={from} onChange={(e) => setFrom(e.target.value)} />
        </Field>
        <Field label="По">
          <input type="date" value={to} onChange={(e) => setTo(e.target.value)} />
        </Field>
      </div>
      <ErrorBox error={report.error} />
      {report.loading && !report.data ? (
        <Loading />
      ) : rows.length === 0 ? (
        <Empty>За этот период подарков не было</Empty>
      ) : (
        <>
          <Table head={['Подарок', 'Сколько', 'В чеках', 'Себестоимость']}>
            {rows.map((r) => (
              <tr key={r.product_id}>
                <td className="px-2 py-2 font-medium">{r.name}</td>
                <td className="whitespace-nowrap px-2 py-2">{r.unit === 'ml' ? formatLiters(r.qty) : `${r.qty} шт`}</td>
                <td className="px-2 py-2">{r.checks}</td>
                <td className="px-2 py-2 md:text-right">
                  <Money value={r.cost_tyiyn} />
                </td>
              </tr>
            ))}
          </Table>
          <div className="text-right font-semibold">Подарки обошлись в {formatSom(total)} — это уже вычтено из валовой прибыли.</div>
        </>
      )}
    </Card>
  )
}

/** Справочник подарков: к товару привязан список того, что можно подарить (SPEC-11). */
export default function Gifts() {
  const list = useLoad(() => get<GiftRule[]>('/gift-rules'), [])
  const [trigger, setTrigger] = useState<Product | null>(null)
  const [items, setItems] = useState<Product[]>([])
  const [minText, setMinText] = useState('')
  const save = useAction()
  const toggle = useAction()

  // Порог «от 3 л» у масла вводится литрами и хранится в мл, у остального — штуками.
  const oil = trigger?.unit === 'ml'
  const minUnits = !minText.trim() ? 0 : oil ? parseLiters(minText) : /^\d+$/.test(minText.trim()) ? Number(minText) : null
  const notFilled = missingWithFocus(
    [minUnits !== null, 'порог числом'],
    [Boolean(trigger), 'товар, к которому дарим'],
    [items.length > 0, 'хотя бы один подарок'],
  )

  const create = () =>
    void save.run(async () => {
      if (!trigger) return
      await post('/gift-rules', {
        trigger_product_id: trigger.id,
        min_units: minUnits ?? 0,
        items: items.map((p) => ({ gift_product_id: p.id, gift_qty: 1 })),
      })
      setTrigger(null)
      setItems([])
      setMinText('')
      list.reload()
    })

  const setActive = (r: GiftRule, active: boolean) =>
    void toggle.run(async () => {
      await patch(`/gift-rules/${r.id}`, { active })
      list.reload()
    })

  return (
    <div>
      <PageHeader title="Подарки" />

      <Card className="mb-4 flex flex-col gap-3">
        <h2 className="text-base font-semibold">Новое правило</h2>
        <Field label="При покупке товара" required>
          {trigger ? (
            <div className="flex items-center justify-between gap-2 rounded-md bg-slate-50 px-3 py-2">
              <span className="font-medium">{trigger.name}</span>
              <button type="button" className="text-slate-400 hover:text-rose-600" onClick={() => setTrigger(null)}>
                ✕
              </button>
            </div>
          ) : (
            <ProductPicker autoFocus={false} placeholder="Найдите товар" onPick={setTrigger} />
          )}
        </Field>
        <Field label={oil ? 'От скольки литров' : 'От скольки штук'} hint="Пусто — с любого количества">
          <input inputMode="decimal" value={minText} onChange={(e) => setMinText(e.target.value)} placeholder={oil ? 'например, 3' : 'например, 2'} />
        </Field>
        <Field label="Можно подарить" required>
          <ProductPicker
            autoFocus={false}
            placeholder="Найдите подарок"
            onPick={(p) => setItems((xs) => (xs.some((x) => x.id === p.id) ? xs : [...xs, p]))}
          />
        </Field>
        {items.length > 0 && (
          <div className="flex flex-wrap gap-2">
            {items.map((p) => (
              <span key={p.id} className="inline-flex items-center gap-1 rounded-md border border-slate-300 px-2 py-1 text-sm">
                {p.name}
                <button type="button" className="text-slate-400 hover:text-rose-600" onClick={() => setItems((xs) => xs.filter((x) => x.id !== p.id))}>
                  ✕
                </button>
              </span>
            ))}
          </div>
        )}
        <Missing items={notFilled} />
        <ErrorBox error={save.error} />
        <div className="flex justify-end">
          <Button disabled={save.busy || notFilled.length > 0} onClick={create}>
            Сохранить правило
          </Button>
        </div>
      </Card>

      <Card>
        <CardTitle>Правила подарков</CardTitle>
        <ErrorBox error={list.error ?? toggle.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : (list.data ?? []).length === 0 ? (
          <Empty>Правил нет. Выберите товар и что к нему можно подарить — касса спросит об этом сама.</Empty>
        ) : (
          <Table head={['При покупке', 'Подарки', 'Статус']}>
            {(list.data ?? []).map((r) => (
              <tr key={r.id} className={r.active ? '' : 'text-slate-400'}>
                <td className="px-2 py-2 font-medium">
                  {r.trigger_name}
                  {r.min_units > 0 && (
                    <span className="ml-1 font-normal text-slate-500">
                      от {r.trigger_unit === 'ml' ? formatLiters(r.min_units) : `${r.min_units} шт.`}
                    </span>
                  )}
                </td>
                <td className="px-2 py-2">{r.items.map((i) => `${i.name} × ${i.gift_qty}`).join(', ') || '—'}</td>
                <td className="px-2 py-2">
                  <DirSwitch checked={r.active} label={`Подарок к «${r.trigger_name}»`} disabled={toggle.busy} onChange={(v) => setActive(r, v)} />
                </td>
              </tr>
            ))}
          </Table>
        )}
      </Card>
      <GiftReport />
    </div>
  )
}
