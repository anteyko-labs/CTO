import { useState } from 'react'
import { ProductPicker } from '../components/ProductPicker'
import { DirSwitch } from '../components/DirSwitch'
import { ExportButton } from '../components/ExportButton'
import { listSheet, periodText, totalBy, unitName, xl, type ExportCol } from '../lib/exportTable'
import { Badge, Button, Card, CardTitle, Empty, ErrorBox, Field, Loading, Missing, Money, PageHeader, Table } from '../components/ui'
import { get, post, patch, qs } from '../lib/api'
import { formatSom, todayBishkek } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import type { GiftRule, Product, StockUnit } from '../lib/types'
import { isKgM, parseQty, qtyText, unitShort } from '../lib/units'
import { giftMinText, giftQtys, giftRuleStatus } from '../lib/giftRule'

interface GiftReportRow {
  product_id: string
  name: string
  unit: StockUnit
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
        {/* Экран только владельца: себестоимость подарков (задача 003 Н13). */}
        <ExportButton
          file="Подарено"
          period={periodText(from, to)}
          disabled={rows.length === 0}
          sheets={() => [
            listSheet('Подарено за период', GIFT_REPORT_COLS, rows, {
              sheet: 'Подарено',
              filter: [`период ${periodText(from, to)}`],
              total: totalBy(GIFT_REPORT_COLS, { Подарок: 'Итого', Себестоимость: xl.money(total) }),
            }),
          ]}
        />
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
                <td className="whitespace-nowrap px-2 py-2">{qtyText(r.unit, r.qty)}</td>
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
  // Количество подарков правила, которое правят: при сохранении не сбрасывается в 1.
  const [qtys, setQtys] = useState<Record<string, number>>({})
  const save = useAction()
  const toggle = useAction()

  // Порог «от 3 л» у масла вводится литрами и хранится в мл, «от 2,5 кг»/«от 1,5 м» — тысячными
  // (г, мм), у остального — штуками (ADR-175).
  const triggerUnit: StockUnit = trigger?.unit ?? 'piece'
  const minUnits = !minText.trim() ? 0 : parseQty(triggerUnit, minText)
  // Подарком товар в кг или м быть не может (задача 006): сервер тоже отказывает.
  const kgMGift = items.find((p) => isKgM(p.unit))
  const notFilled = missingWithFocus(
    [minUnits !== null, 'порог числом'],
    [Boolean(trigger), 'товар, к которому дарим'],
    [items.length > 0, 'хотя бы один подарок'],
    [!kgMGift, `подарок не в кг и не в метрах (уберите «${kgMGift?.name ?? ''}»)`],
  )

  const create = () =>
    void save.run(async () => {
      if (!trigger) return
      await post('/gift-rules', {
        trigger_product_id: trigger.id,
        min_units: minUnits ?? 0,
        items: items.map((p) => ({ gift_product_id: p.id, gift_qty: qtys[p.id] ?? 1 })),
      })
      setTrigger(null)
      setItems([])
      setMinText('')
      setQtys({})
      list.reload()
    })

  // «Поправить» правило (порог округлён при смене единицы): форма заполняется правилом,
  // сохранение заменяет его и включает.
  const edit = (r: GiftRule) =>
    void toggle.run(async () => {
      const [t, ...gifts] = await Promise.all([r.trigger_product_id, ...r.items.map((i) => i.gift_product_id)].map((id) => get<Product>(`/products/${id}`)))
      setTrigger(t)
      setItems(gifts)
      setMinText(giftMinText(r))
      setQtys(giftQtys(r))
      window.scrollTo({ top: 0, behavior: 'smooth' })
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
        <Field label={triggerUnit === 'ml' ? 'От скольки литров' : triggerUnit === 'piece' ? 'От скольки штук' : `От скольки ${unitShort(triggerUnit)}`} hint="Пусто — с любого количества">
          <input
            inputMode="decimal"
            value={minText}
            onChange={(e) => setMinText(e.target.value)}
            placeholder={triggerUnit === 'piece' ? 'например, 2' : 'например, 3'}
          />
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
        <CardTitle
          actions={
            <ExportButton
              file="Правила подарков"
              disabled={(list.data ?? []).length === 0}
              sheets={() => [listSheet('Правила подарков', GIFT_RULE_COLS, list.data ?? [], { sheet: 'Правила' })]}
            />
          }
        >
          Правила подарков
        </CardTitle>
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
                      от {qtyText(r.trigger_unit, r.min_units)}
                    </span>
                  )}
                </td>
                <td className="px-2 py-2">{r.items.map((i) => `${i.name} × ${i.gift_qty}`).join(', ') || '—'}</td>
                <td className="px-2 py-2">
                  {r.needs_fix ? (
                    <div className="flex flex-wrap items-center gap-2" data-testid="gift-rule-fix">
                      <Badge tone="amber">поправьте порог</Badge>
                      <Button variant="secondary" className="px-2 py-1 text-xs" disabled={toggle.busy} onClick={() => edit(r)}>
                        Поправить
                      </Button>
                      <div className="w-full text-xs text-slate-500">Единица товара сменилась, порог округлён. Проверьте и сохраните — правило включится.</div>
                    </div>
                  ) : (
                    <DirSwitch checked={r.active} label={`Подарок к «${r.trigger_name}»`} disabled={toggle.busy} onChange={(v) => setActive(r, v)} />
                  )}
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

const GIFT_REPORT_COLS: ExportCol<GiftReportRow>[] = [
  { label: 'Подарок', value: (r) => r.name },
  { label: 'Сколько', value: (r) => xl.units(r.qty, r.unit) },
  { label: 'Ед.', value: (r) => unitName(r.unit) },
  { label: 'В чеках', value: (r) => xl.int(r.checks) },
  { label: 'Себестоимость', value: (r) => xl.money(r.cost_tyiyn) },
]

const GIFT_RULE_COLS: ExportCol<GiftRule>[] = [
  { label: 'При покупке', value: (r) => r.trigger_name },
  { label: 'От', value: (r) => (r.min_units > 0 ? xl.units(r.min_units, r.trigger_unit) : null) },
  { label: 'Ед.', value: (r) => (r.min_units > 0 ? unitName(r.trigger_unit) : '') },
  { label: 'Подарки', value: (r) => r.items.map((i) => `${i.name} × ${i.gift_qty}`).join(', ') },
  { label: 'Статус', value: (r) => giftRuleStatus(r) },
]
