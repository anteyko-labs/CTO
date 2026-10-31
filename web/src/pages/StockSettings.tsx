// «Настройки → Склад» (только владелец): какая цена масла открывается первой — за канистру или за
// литр — в новой строке прихода, оприходования и в карточке нового товара (задача 003, Н7, ADR-150);
// срок залежалости (ADR-039). Изменение — в журнал действий `settings.stock` (I5).
import { useState } from 'react'
import { Link } from 'react-router-dom'
import { Button, Card, CardTitle, ErrorBox, Field, Loading, PageHeader, toast } from '../components/ui'
import { get, put } from '../lib/api'
import { useAction, useLoad } from '../lib/hooks'
import type { QtyMode } from '../lib/receiptLines'
import type { StockSettings } from '../lib/stockSettings'

const MODES: { value: QtyMode; label: string }[] = [
  { value: 'can', label: 'За канистру — количество канистрами' },
  { value: 'l', label: 'За литр — количество литрами' },
]

type ModeKey = 'oil_price_mode_receipt' | 'oil_price_mode_stock_in'

export default function StockSettingsPage() {
  const st = useLoad(() => get<StockSettings>('/settings/stock'), [])
  const act = useAction()
  const [days, setDays] = useState('')
  const s = st.data

  // Выбор меняется сразу; не сохранилось — экран перечитывает настройку с сервера.
  const save = (patch: Partial<StockSettings>, done: string) => {
    st.setData((prev) => prev && { ...prev, ...patch })
    void act.run(async () => {
      try {
        st.setData(await put<StockSettings>('/settings/stock', patch))
        toast(done)
      } catch (e) {
        st.reload()
        throw e
      }
    })
  }

  const modeCard = (key: ModeKey, title: string, text: string) =>
    s && (
      <Card data-testid={`stock-${key}`}>
        <CardTitle>{title}</CardTitle>
        <div className="flex flex-col gap-2" role="radiogroup" aria-label={title}>
          {MODES.map((m) => (
            <label key={m.value} className="inline-flex items-center gap-2 text-sm">
              <input
                type="radio"
                name={key}
                className="h-4 w-4"
                checked={s[key] === m.value}
                disabled={act.busy}
                onChange={() => save({ [key]: m.value }, `${title}: ${m.value === 'can' ? 'за канистру' : 'за литр'}`)}
              />
              {m.label}
            </label>
          ))}
        </div>
        <p className="mt-2 text-sm text-slate-500">{text}</p>
      </Card>
    )

  return (
    <div>
      <PageHeader title="Склад" subtitle="Приход, оприходование и залежалый товар" back={<Link to="/settings">← Настройки</Link>} />
      <ErrorBox error={st.error ?? act.error} />
      {!s ? (
        <Loading />
      ) : (
        <div className="space-y-4">
          {modeCard(
            'oil_price_mode_receipt',
            'Цена масла в приходе',
            'Какой открывается новая строка масла в приходе и цена прихода в карточке нового товара. Вторая цена всё равно видна и вводится: канистра ↔ литр считаются сами.',
          )}
          {modeCard(
            'oil_price_mode_stock_in',
            'Цена масла в оприходовании',
            'Какой открывается новая строка масла в оприходовании без поставщика и карточка нового товара оттуда.',
          )}
          <Card data-testid="stock-stale-days">
            <CardTitle>Залежалый товар</CardTitle>
            <div className="flex flex-wrap items-end gap-2">
              <Field label="Без продаж, дней" hint={`Сейчас ${s.stale_days}. От 1 до 3650`}>
                <input className="w-28" inputMode="numeric" placeholder={String(s.stale_days)} value={days} onChange={(e) => setDays(e.target.value)} />
              </Field>
              <Button
                variant="secondary"
                disabled={act.busy || !/^\d+$/.test(days.trim())}
                onClick={() => {
                  save({ stale_days: Number(days) }, 'Срок залежалости сохранён')
                  setDays('')
                }}
              >
                Сохранить
              </Button>
            </div>
          </Card>
        </div>
      )}
    </div>
  )
}
