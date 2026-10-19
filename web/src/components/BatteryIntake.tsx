// Приём старых аккумуляторов по весу: деньги из кассы смены, на склад — граммы (ADR-048).
// Владелец исправляет проведённый приём той же формой (SPEC-29 §24, ADR-085).
import { useState } from 'react'
import { get, newOpId, post } from '../lib/api'
import { correctionMarks, loadCorrections } from '../lib/corrections'
import { amountInput, fixedText, qtyInput } from '../lib/docFix'
import { formatDateTime, formatKg, formatSom, parseLiters, somInput } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import { divRound } from '../lib/money'
import { CorrectionBadges, FixButton } from './Correction'
import { FIX_SUBMIT, FixBanner, FixDraft, type FixMode } from './FixDraft'
import { Pager, usePager } from './Pager'
import { Button, ErrorBox, Field, Loading, Missing, Modal, toast } from './ui'

export interface BatteryInfo {
  product_id: string
  name: string
  stock_g: number
  avg_per_kg_tyiyn: number | null
  intake_per_kg_tyiyn: number
  sale_per_kg_tyiyn: number
}

interface IntakeRow {
  id: string
  number: number
  grams: number
  price_per_kg_tyiyn: number
  amount_tyiyn: number
  comment: string
  user_name: string
  created_at: string
  reversed: boolean
}

/** Приём в виде запроса проведения (без op_id). */
interface IntakeDoc {
  grams: number
  price_per_kg_tyiyn: number
  comment: string
}

/** Приёмы, которые можно отменить: администратору — своей открытой смены, владельцу — последние.
 *  Отмена снимает вес со склада и возвращает деньги в кассу (ADR-065). */
function IntakeList({ onChanged, onFix, tick }: { onChanged: () => void; onFix: (r: IntakeRow) => void; tick: number }) {
  const list = useLoad(() => get<IntakeRow[]>('/batteries/intakes'), [tick])
  // Отметки «исправлен → / исправление» — одним запросом на список.
  const marks = useLoad(() => loadCorrections('battery_intake').then(correctionMarks), [tick])
  const act = useAction()
  const reverse = (r: IntakeRow) =>
    void act.run(async () => {
      const reason = window.prompt(`Отменить приём № ${r.number}: ${formatKg(r.grams)}, ${formatSom(r.amount_tyiyn)} вернутся в кассу. Причина:`)
      if (reason === null) return
      await post(`/batteries/intakes/${r.id}/reverse`, { op_id: newOpId(), comment: reason })
      toast(`Приём № ${r.number} отменён: ${formatSom(r.amount_tyiyn)} вернулись в кассу`)
      list.reload()
      onChanged()
    })
  const rows = list.data ?? []
  const pager = usePager(rows, 'battery.intakes')
  if (rows.length === 0) return <ErrorBox error={list.error} />
  return (
    <div className="border-t border-slate-200 pt-3">
      <div className="mb-2 text-sm font-semibold">Приёмы этой смены</div>
      <ErrorBox error={act.error ?? list.error} />
      <ul className="flex flex-col gap-1 text-sm">
        {pager.shown.map((r) => (
          <li key={r.id} className="flex flex-wrap items-center justify-between gap-2">
            <span className={r.reversed ? 'text-slate-400 line-through' : ''}>
              № {r.number} · {formatDateTime(r.created_at)} · {formatKg(r.grams)} · {formatSom(r.amount_tyiyn)}
              {r.comment && ` · ${r.comment}`}
            </span>{' '}
            <CorrectionBadges mark={marks.data?.get(r.id)} />
            {r.reversed ? (
              <span className="text-xs text-slate-500">отменён</span>
            ) : (
              <span className="flex gap-1">
                <FixButton small onClick={() => onFix(r)} />
                <Button variant="secondary" className="px-2 py-1 text-xs" disabled={act.busy} onClick={() => reverse(r)}>
                  Отменить приём
                </Button>
              </span>
            )}
          </li>
        ))}
      </ul>
      <Pager page={pager.page} size={pager.size} total={pager.total} onPage={pager.setPage} onSize={pager.setSize} label="Страницы приёмов аккумуляторов" />
    </div>
  )
}

/** Форма приёма: вес и цена за кг; с `fix` — исправление проведённого приёма. */
function IntakeForm({ info, fix, onCancel, onDone }: { info: BatteryInfo | null; fix?: FixMode<IntakeDoc>; onCancel: () => void; onDone: () => void }) {
  const [kg, setKg] = useState(fix ? qtyInput('g', fix.doc.grams) : '')
  const [price, setPrice] = useState<string | null>(fix ? amountInput(fix.doc.price_per_kg_tyiyn) : null)
  const [comment, setComment] = useState(fix?.doc.comment ?? '')
  // Один op_id на открытую форму: повтор после потерянного ответа не выдаст деньги дважды.
  const [opId] = useState(newOpId)
  const { busy, error, run } = useAction()

  const priceText = price ?? (info ? somInput(info.intake_per_kg_tyiyn) : '')
  // Килограммы с точностью до грамма: «12,35» → 12 350 г.
  const grams = kg.trim() ? parseLiters(kg) : null
  const perKg = (() => {
    const t = priceText.trim().replace(/\s/g, '').replace(',', '.')
    if (!/^\d+(\.\d{1,2})?$/.test(t)) return null
    const [a, b = ''] = t.split('.')
    return Number(a) * 100 + Number(b.padEnd(2, '0'))
  })()
  const amount = grams !== null && perKg !== null ? divRound(grams * perKg, 1000) : null
  const notFilled = missingWithFocus(
    [grams !== null && grams > 0, 'вес', '#battery-kg'],
    [perKg !== null, 'цену за кг', '#battery-price'],
    [!fix || Boolean(fix.comment.trim()), 'что и почему исправляете', '#correction-comment'],
  )

  const save = () =>
    void run(async () => {
      if (grams === null || perKg === null) return
      if (fix) {
        const out = await fix.save({ ...fix.doc, grams, price_per_kg_tyiyn: perKg, comment })
        if (!out) return
        toast(fixedText(out))
        onDone()
        return
      }
      const out = await post<{ amount_tyiyn: number; number: number }>('/batteries/intake', {
        op_id: opId,
        grams,
        price_per_kg_tyiyn: perKg,
        comment,
      })
      toast(`Приём № ${out.number}: выдано из кассы ${formatSom(out.amount_tyiyn)}`)
      onDone()
    })

  return (
    <div className="flex flex-col gap-3">
      {fix && <FixBanner fix={fix} />}
      <div className="grid grid-cols-2 gap-3">
        <Field label="Вес, кг" required hint="Можно с граммами: 12,35">
          <input id="battery-kg" autoFocus={!fix} inputMode="decimal" value={kg} onChange={(e) => setKg(e.target.value)} />
        </Field>
        <Field label="Цена за кг, с" required>
          <input id="battery-price" inputMode="decimal" value={priceText} onChange={(e) => setPrice(e.target.value)} />
        </Field>
      </div>
      {amount !== null && grams !== null && (
        <div className="rounded-md bg-emerald-50 px-3 py-2 text-sm text-emerald-900">
          {formatKg(grams)} × {formatSom(perKg ?? 0)} = <b>выдать {formatSom(amount)}</b> из кассы
        </div>
      )}
      {info && (
        <div className="text-xs text-slate-500">
          На складе {formatKg(info.stock_g)}
          {info.avg_per_kg_tyiyn !== null && `, средняя закупка ${formatSom(info.avg_per_kg_tyiyn)} за кг`}.
        </div>
      )}
      <Field label="Комментарий">
        <input value={comment} onChange={(e) => setComment(e.target.value)} placeholder="например, 2 АКБ 60 Ач" />
      </Field>
      <Missing items={notFilled} />
      <ErrorBox error={error ?? fix?.error} />
      <div className="flex justify-end gap-2">
        <Button variant="secondary" onClick={onCancel}>
          Отмена
        </Button>
        <Button disabled={busy || Boolean(fix?.busy) || notFilled.length > 0} onClick={save}>
          {fix ? FIX_SUBMIT : 'Принять и выдать деньги'}
        </Button>
      </div>
    </div>
  )
}

export function BatteryIntake({ onClose }: { onClose: () => void }) {
  const info = useLoad(() => get<BatteryInfo>('/batteries'), [])
  const [fixing, setFixing] = useState<IntakeRow | null>(null)
  const [tick, setTick] = useState(0)

  return (
    <Modal title={fixing ? `Исправить приём аккумуляторов № ${fixing.number}` : 'Приём аккумуляторов по весу'} onClose={onClose}>
      {info.loading && !info.data ? (
        <Loading />
      ) : (
        <div className="flex flex-col gap-3">
          {fixing ? (
            <FixDraft<IntakeDoc> key={fixing.id} kind="battery_intake" id={fixing.id} onCancel={() => setFixing(null)}>
              {(fix) => (
                <IntakeForm
                  info={info.data}
                  fix={fix}
                  onCancel={() => setFixing(null)}
                  onDone={() => {
                    setFixing(null)
                    setTick((t) => t + 1)
                    info.reload()
                  }}
                />
              )}
            </FixDraft>
          ) : (
            <IntakeForm info={info.data} onCancel={onClose} onDone={onClose} />
          )}
          <ErrorBox error={info.error} />
          <IntakeList onChanged={info.reload} onFix={setFixing} tick={tick} />
        </div>
      )}
    </Modal>
  )
}
