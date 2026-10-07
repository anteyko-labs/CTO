import { useState } from 'react'
import { ProductPicker } from '../components/ProductPicker'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, Missing, PageHeader, Table } from '../components/ui'
import { get, post, patch } from '../lib/api'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import type { GiftRule, Product } from '../lib/types'

/** Справочник подарков: к товару привязан список того, что можно подарить (SPEC-11). */
export default function Gifts() {
  const list = useLoad(() => get<GiftRule[]>('/gift-rules'), [])
  const [trigger, setTrigger] = useState<Product | null>(null)
  const [items, setItems] = useState<Product[]>([])
  const save = useAction()
  const toggle = useAction()

  const notFilled = missingWithFocus(
    [Boolean(trigger), 'товар, к которому дарим'],
    [items.length > 0, 'хотя бы один подарок'],
  )

  const create = () =>
    void save.run(async () => {
      if (!trigger) return
      await post('/gift-rules', {
        trigger_product_id: trigger.id,
        items: items.map((p) => ({ gift_product_id: p.id, gift_qty: 1 })),
      })
      setTrigger(null)
      setItems([])
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
        <ErrorBox error={list.error ?? toggle.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : (list.data ?? []).length === 0 ? (
          <Empty>Правил нет. Выберите товар и что к нему можно подарить — касса спросит об этом сама.</Empty>
        ) : (
          <Table head={['При покупке', 'Подарки', 'Статус', '']}>
            {(list.data ?? []).map((r) => (
              <tr key={r.id} className={r.active ? '' : 'text-slate-400'}>
                <td className="px-2 py-2 font-medium">{r.trigger_name}</td>
                <td className="px-2 py-2">{r.items.map((i) => `${i.name} × ${i.gift_qty}`).join(', ') || '—'}</td>
                <td className="px-2 py-2">{r.active ? <Badge tone="green">активно</Badge> : <Badge>отключено</Badge>}</td>
                <td className="px-2 py-2 text-right">
                  <Button variant="secondary" className="px-2 py-1 text-xs" disabled={toggle.busy} onClick={() => setActive(r, !r.active)}>
                    {r.active ? 'Отключить' : 'Включить'}
                  </Button>
                </td>
              </tr>
            ))}
          </Table>
        )}
      </Card>
    </div>
  )
}
