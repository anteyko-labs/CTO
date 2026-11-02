// Окно «Скидка» на кассе (задача 003 Н3, ADR-147): на весь чек — процентом (быстрые из «Настройки →
// Касса») или суммой, на строку — новой ценой или процентом; «Убрать скидку» — цены по прайсу. Скидка —
// цены строк ниже прайса (ADR-097), раскладка и округление — `lib/discount.ts`. До проведения меняется.
// Право «Скидка в чеке» и предел администратора проверяет сервер (I12).
import { useState } from 'react'
import { useNumField } from '../lib/cashierMode'
import { checkDiscount, linePercentPrice, lineSum, overLimit, percentDiscount, spreadDiscount, type DiscLine } from '../lib/discount'
import { formatSom, parseSom } from '../lib/format'
import { Button, Field, Modal, Notice } from './ui'

export type DiscountLine = DiscLine & { name: string }

export function DiscountModal({
  lines,
  quick,
  services,
  limit,
  touch,
  onApply,
  onClose,
}: {
  lines: DiscountLine[]
  quick: number[]
  services: boolean
  /** Предел администратора; владельцу — `null`. */
  limit: { pct: number | null; tyiyn: number | null } | null
  touch: boolean
  /** Новые цены строк, тыйын, по ключу строки. */
  onApply: (prices: Map<string, number>) => void
  onClose: () => void
}) {
  const num = useNumField()
  const [tab, setTab] = useState<'check' | 'line'>('check')
  const [pct, setPct] = useState('')
  const [sum, setSum] = useState('')
  const [lineKey, setLineKey] = useState(() => lines.find((l) => !l.gift)?.key ?? '')
  const [linePrice, setLinePrice] = useState('')
  const [linePct, setLinePct] = useState('')
  const big = touch ? 'min-h-[64px]! text-lg!' : ''

  // Заказанная скидка на весь чек: процент от прайса строк под скидку или сумма.
  const pctValue = /^\d{1,2}$/.test(pct.trim()) ? Number(pct) : null
  const sumValue = sum.trim() ? parseSom(sum) : null
  const wanted = pctValue !== null && pctValue > 0 ? percentDiscount(lines, pctValue, { services }) : sumValue
  const spread = wanted !== null && wanted > 0 ? spreadDiscount(lines, wanted, { services }) : null

  const line = lines.find((l) => l.key === lineKey)
  const lpct = /^\d{1,2}$/.test(linePct.trim()) ? Number(linePct) : null
  const lprice = line ? (lpct !== null && lpct > 0 ? linePercentPrice(line.list, lpct) : linePrice.trim() ? parseSom(linePrice) : null) : null

  // Что выйдет: скидка чека после применения и превышение предела администратора.
  const after = (prices: Map<string, number>) => checkDiscount(lines.map((l) => ({ ...l, price: prices.get(l.key) ?? l.price })))
  const preview = tab === 'check' ? (spread ? after(spread.prices) : null) : line && lprice !== null ? after(new Map([[line.key, lprice]])) : null
  const over = preview && limit ? overLimit(preview.discount, preview.list, limit) : null
  const limitText = limit ? [limit.pct !== null ? `${limit.pct} %` : '', limit.tyiyn !== null ? formatSom(limit.tyiyn) : ''].filter(Boolean).join(' и ') : ''
  const now = checkDiscount(lines)

  const apply = () => {
    if (tab === 'check' && spread) onApply(spread.prices)
    if (tab === 'line' && line && lprice !== null) onApply(new Map([[line.key, lprice]]))
  }
  // Строки ниже прайса — снова по прайсу; поднятая цена остаётся.
  const clear = () => onApply(new Map(lines.filter((l) => !l.gift && l.price < l.list).map((l) => [l.key, l.list])))

  return (
    <Modal title="Скидка" onClose={onClose}>
      <div className="flex flex-col gap-3" data-testid="discount-modal">
        <div className="grid grid-cols-2 overflow-hidden rounded-md border border-slate-300">
          {(['check', 'line'] as const).map((t) => (
            <button
              key={t}
              type="button"
              aria-pressed={tab === t}
              className={`py-2 font-medium ${touch ? 'min-h-[56px] text-lg' : 'min-h-11'} ${tab === t ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
              onClick={() => setTab(t)}
            >
              {t === 'check' ? 'На весь чек' : 'На строку'}
            </button>
          ))}
        </div>
        {tab === 'check' ? (
          <>
            {quick.length > 0 && (
              <div className="grid grid-cols-3 gap-2 sm:grid-cols-5">
                {quick.map((q) => (
                  <Button
                    key={q}
                    variant={pctValue === q ? 'primary' : 'secondary'}
                    className={big}
                    onClick={() => {
                      setPct(String(q))
                      setSum('')
                    }}
                  >
                    {q} %
                  </Button>
                ))}
              </div>
            )}
            <div className="grid grid-cols-2 gap-3">
              <Field label="Процент">
                <input
                  id="discount-pct"
                  {...num('int')}
                  maxLength={2}
                  value={pct}
                  onChange={(e) => {
                    setPct(e.target.value.replace(/\D/g, '').slice(0, 2))
                    setSum('')
                  }}
                />
              </Field>
              <Field label="или сумма, с">
                <input
                  id="discount-sum"
                  {...num('decimal')}
                  value={sum}
                  onChange={(e) => {
                    setSum(e.target.value)
                    setPct('')
                  }}
                />
              </Field>
            </div>
            {!services && <div className="text-sm text-slate-500">Услуги — по прайсу (настройка «Касса»).</div>}
          </>
        ) : (
          <>
            <div className="flex max-h-[40dvh] flex-col gap-1 overflow-y-auto">
              {lines
                .filter((l) => !l.gift)
                .map((l) => (
                  <button
                    key={l.key}
                    type="button"
                    aria-pressed={l.key === lineKey}
                    className={`flex items-center justify-between gap-2 rounded-md border px-3 text-left ${touch ? 'min-h-[56px] text-lg' : 'min-h-11'} ${
                      l.key === lineKey ? 'border-sky-600 bg-sky-50' : 'border-slate-300 bg-white'
                    }`}
                    onClick={() => {
                      setLineKey(l.key)
                      setLinePrice('')
                      setLinePct('')
                    }}
                  >
                    <span className="min-w-0 truncate">{l.name}</span>
                    <span className="shrink-0 tabular-nums">
                      {formatSom(l.price)}
                      {l.price !== l.list && <span className="ml-1 text-sm text-slate-500">прайс {formatSom(l.list)}</span>}
                    </span>
                  </button>
                ))}
            </div>
            {line && (
              <div className="grid grid-cols-2 gap-3">
                <Field label={line.kind === 'pour' ? 'Новая цена за л, с' : line.kind === 'weight' ? 'Новая цена за кг, с' : line.kind === 'length' ? 'Новая цена за м, с' : 'Новая цена, с'}>
                  <input
                    id="discount-line-price"
                    {...num('decimal')}
                    value={linePrice}
                    onChange={(e) => {
                      setLinePrice(e.target.value)
                      setLinePct('')
                    }}
                  />
                </Field>
                <Field label="или процент">
                  <input
                    id="discount-line-pct"
                    {...num('int')}
                    maxLength={2}
                    value={linePct}
                    onChange={(e) => {
                      setLinePct(e.target.value.replace(/\D/g, '').slice(0, 2))
                      setLinePrice('')
                    }}
                  />
                </Field>
              </div>
            )}
          </>
        )}
        {preview && (
          <div className="rounded-md bg-slate-50 px-3 py-2 text-base" data-testid="discount-preview">
            Скидка <b>−{formatSom(preview.discount)}</b> · итого <b>{formatSom(preview.total)}</b>
            {tab === 'line' && line && lprice !== null && (
              <span className="block text-sm text-slate-600">
                {line.name}: {formatSom(lprice)} вместо {formatSom(line.list)}, по строке {formatSom(lineSum(line.kind, line.qty, lprice))}
              </span>
            )}
          </div>
        )}
        {over && <Notice tone="rose">Больше предела администратора ({limitText}) — уменьшите скидку или попросите владельца.</Notice>}
        {limit && !over && limitText && <div className="text-sm text-slate-500">Предел скидки администратора: {limitText} на чек.</div>}
        <div className="text-xs text-slate-500">Дешевле закупки продать нельзя — сервер не проведёт такую строку.</div>
        <div className="grid grid-cols-2 gap-2">
          <Button variant="secondary" className={big} disabled={now.discount === 0} onClick={clear}>
            Убрать скидку
          </Button>
          <Button className={big} disabled={!preview || over !== null} onClick={apply}>
            Применить
          </Button>
        </div>
      </div>
    </Modal>
  )
}
