// Перелив масла: остаток из одного масла в другое, себестоимость получателя — средневзвешенная (SPEC-14).
// Владелец исправляет проведённый перелив той же формой (SPEC-29 §24, ADR-085).
import { useState } from 'react'
import { CorrectionBadges, FixButton } from '../components/Correction'
import { FIX_SUBMIT, FixBanner, FixDraft, type FixMode } from '../components/FixDraft'
import { Pager, usePager } from '../components/Pager'
import { Button, Card, Empty, ErrorBox, Field, Loading, Missing, Modal, Notice, PageHeader, Table, toast } from '../components/ui'
import { get, newOpId, patch, post } from '../lib/api'
import { formatDateTime, formatLiters, formatSom, litersInput, parseLiters, parseSom, somInput } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import { canisterPourPrice, divRound } from '../lib/money'
import { useUser } from '../lib/auth'
import { correctionMarks, loadCorrections } from '../lib/corrections'
import { fixedText, oilStockForFix } from '../lib/docFix'
import type { Product } from '../lib/types'

interface OilState {
  product_id: string
  name: string
  stock_ml: number
  /** Стоимость и средняя — только владельцу (ADR-082). */
  value_tyiyn?: number
  avg_per_l_tyiyn?: number | null
}

interface TransferOut {
  id: string
  number: number
  qty_ml: number
  value_tyiyn?: number
  from: OilState
  to: OilState
}

interface TransferRow {
  id: string
  number: number
  from_name: string
  to_name: string
  qty_ml: number
  value_tyiyn?: number
  comment: string
  user_name: string
  created_at: string
  reversed: boolean
}

/** Себестоимость литра для показа: стоимость остатка × 1000 / мл. */
const perLiter = (value: number, ml: number): number | null => (ml > 0 ? divRound(value * 1000, ml) : null)


/** Перелив в виде запроса проведения (без op_id). */
interface TransferDoc {
  from_product_id: string
  to_product_id: string
  qty_ml: number
  comment: string
}

/**
 * Форма перелива; с `fix` — исправление проведённого: исходный перелив сторнируется в той же
 * операции, поэтому его литры считаются вернувшимися в источник.
 */
function TransferForm({ oils, fix, onPosted, onCancel }: { oils: Product[]; fix?: FixMode<TransferDoc>; onPosted: (out: TransferOut | null) => void; onCancel?: () => void }) {
  const [form, setForm] = useState(
    fix
      ? { from: fix.doc.from_product_id, to: fix.doc.to_product_id, liters: litersInput(fix.doc.qty_ml), comment: fix.doc.comment }
      : { from: '', to: '', liters: '', comment: '' },
  )
  const [opId, setOpId] = useState(newOpId)
  // Перелив отменяет только владелец — перед проведением показываем, что и куда (ui-rules §3).
  const [confirming, setConfirming] = useState(false)
  const act = useAction()
  const stockOf = (p: Product) => oilStockForFix(p.id, p.stock_qty, fix ? fix.doc : null)

  const from = oils.find((p) => p.id === form.from)
  const to = oils.find((p) => p.id === form.to)
  const ml = form.liters.trim() ? parseLiters(form.liters) : null
  const fromStock = from ? stockOf(from) : 0

  // Предпросмотр: стоимость уходит по средней источника, у получателя — средневзвешенная.
  // В исправлении не показывается: остатки ещё включают исходный перелив.
  const preview = (() => {
    if (fix || !from || !to || ml === null || ml <= 0 || from.stock_value_tyiyn === undefined || to.stock_value_tyiyn === undefined) return null
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
    [!from || ml === null || ml <= fromStock, `не больше остатка: ${from ? formatLiters(Math.max(fromStock, 0)) : ''}`, '#oil-liters'],
    [!fix || Boolean(fix.comment.trim()), 'что и почему исправляете', '#correction-comment'],
  )

  const submit = () =>
    void act.run(async () => {
      if (ml === null) return
      if (fix) {
        const out = await fix.save({ ...fix.doc, from_product_id: form.from, to_product_id: form.to, qty_ml: ml, comment: form.comment })
        if (!out) return
        toast(fixedText(out))
        onPosted(null)
        return
      }
      const out = await post<TransferOut>('/oil/transfers', {
        op_id: opId,
        from_product_id: form.from,
        to_product_id: form.to,
        qty_ml: ml,
        comment: form.comment,
      })
      setOpId(newOpId())
      setForm({ from: '', to: '', liters: '', comment: '' })
      toast(`Перелито ${formatLiters(out.qty_ml)}`)
      onPosted(out)
    })

  return (
    <>
      {fix && <FixBanner fix={fix} />}
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label="Из масла" required>
          <select
            id="oil-from"
            value={form.from}
            onChange={(e) => {
              // Количество не подставляется: «весь остаток» — отдельной кнопкой (проверка 20.10.2026).
              setForm({ ...form, from: e.target.value, liters: '' })
            }}
          >
            <option value="">— выберите —</option>
            {oils
              .filter((p) => stockOf(p) > 0)
              .map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name} · {formatLiters(stockOf(p))}
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
                  {p.name} · {formatLiters(Math.max(stockOf(p), 0))}
                </option>
              ))}
          </select>
        </Field>
      </div>
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label="Сколько, л" required>
          <input id="oil-liters" inputMode="decimal" value={form.liters} onChange={(e) => setForm({ ...form, liters: e.target.value })} />
          {from && fromStock > 0 && (
            <button type="button" className="mt-1 self-start text-xs text-sky-700 underline" onClick={() => setForm({ ...form, liters: litersInput(fromStock) })}>
              весь остаток: {formatLiters(fromStock)}
            </button>
          )}
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
      <ErrorBox error={act.error ?? fix?.error} />
      <div className="flex justify-end gap-2">
        {onCancel && (
          <Button variant="secondary" onClick={onCancel}>
            Отмена
          </Button>
        )}
        <Button disabled={act.busy || Boolean(fix?.busy) || notFilled.length > 0} onClick={() => (fix ? submit() : setConfirming(true))}>
          {fix ? FIX_SUBMIT : 'Перелить'}
        </Button>
      </div>
      {confirming && from && to && ml !== null && (
        <Modal title="Перелить масло?" onClose={() => setConfirming(false)}>
          <div className="flex flex-col gap-3 text-sm">
            <p>
              <b>{formatLiters(ml)}</b> из «{from.name}» в «{to.name}».
            </p>
            {viscosityOf(from) && viscosityOf(to) && viscosityOf(from) !== viscosityOf(to) && (
              <Notice tone="rose">
                Разная вязкость: {viscosityOf(from)} → {viscosityOf(to)}. Проверьте, то ли масло выбрано.
              </Notice>
            )}
            <p className="text-slate-600">Отменить перелив сможет только владелец.</p>
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setConfirming(false)}>
                Назад
              </Button>
              <Button
                disabled={act.busy}
                onClick={() => {
                  setConfirming(false)
                  submit()
                }}
              >
                Перелить
              </Button>
            </div>
          </div>
        </Modal>
      )}
    </>
  )
}

/** Вязкость масла из характеристик: «5W-30». */
const viscosityOf = (p: Product): string => String((p.attrs as Record<string, string> | null)?.viscosity ?? '').trim()

// Перелив проводит владелец или администратор с правом «Перелив масла» (ADR-082): администратор
// себестоимости не видит (сервер её не отдаёт), цену не меняет и перелив не отменяет.
export default function OilTransfer() {
  const owner = useUser().role === 'owner'
  const products = useLoad(() => get<Product[]>('/products?limit=500'), [])
  const history = useLoad(() => get<TransferRow[]>('/oil/transfers'), [])
  const historyPager = usePager(history.data ?? [], 'oil.transfers')
  // Отметки «исправлен → / исправление» — одним запросом на список; администратору не нужны.
  const marks = useLoad(() => (owner ? loadCorrections('oil_transfer').then(correctionMarks) : Promise.resolve(null)), [owner])
  const [fixing, setFixing] = useState<TransferRow | null>(null)
  const [done, setDone] = useState<TransferOut | null>(null)
  const [prices, setPrices] = useState({ canister: '' })
  const priceAct = useAction()
  const [reversing, setReversing] = useState<{ row: TransferRow; comment: string; opId: string } | null>(null)
  const revAct = useAction()

  const oils = (products.data ?? []).filter((p) => p.unit === 'ml' && !p.archived)

  const posted = (out: TransferOut | null) => {
    if (out && owner) setDone(out)
    if (out) setPrices({ canister: somInput(oils.find((p) => p.id === out.to.product_id)?.sale_price_tyiyn ?? 0) })
    if (!out) setFixing(null)
    products.reload()
    history.reload()
    marks.reload()
  }

  const savePrices = () =>
    void priceAct.run(async () => {
      if (!done) return
      const canister = parseSom(prices.canister)
      if (canister === null) throw new Error('Неверная цена')
      await patch(`/products/${done.to.product_id}/prices`, { sale_price_tyiyn: canister })
      setDone(null)
      products.reload()
      toast('Цены обновлены')
    })

  const doneProduct = done ? oils.find((p) => p.id === done.to.product_id) : undefined
  // Розлив за литр — от цены канистры (ADR-074), отдельно не задаётся.
  const newCanister = parseSom(prices.canister)
  const newPour = doneProduct?.container_ml && newCanister !== null ? canisterPourPrice(newCanister, doneProduct.container_ml) : null

  return (
    <div className="flex flex-col gap-4">
      <PageHeader title="Перелив масла" />
      {!owner && (
        <div className="text-sm text-slate-600" data-testid="oil-admin-note">
          Владелец получит уведомление о переливе. Отменить перелив и поменять цену может только он.
        </div>
      )}
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
            {fixing && <h2 className="font-semibold">Исправить перелив № {fixing.number}</h2>}
            {fixing ? (
              <FixDraft<TransferDoc> key={fixing.id} kind="oil_transfer" id={fixing.id} onCancel={() => setFixing(null)}>
                {(fix) => <TransferForm oils={oils} fix={fix} onPosted={posted} onCancel={() => setFixing(null)} />}
              </FixDraft>
            ) : (
              <TransferForm oils={oils} onPosted={posted} />
            )}
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
              {typeof done.to.avg_per_l_tyiyn === 'number' && `, себестоимость ${formatSom(done.to.avg_per_l_tyiyn)} за литр`}.
              {doneProduct?.container_ml && typeof done.to.avg_per_l_tyiyn === 'number' &&
                ` Канистра ${formatLiters(doneProduct.container_ml)} обходится в ${formatSom(divRound(done.to.avg_per_l_tyiyn * doneProduct.container_ml, 1000))}.`}
            </div>
          </div>
          <div className="text-sm font-medium">Цена продажи «{done.to.name}»</div>
          <div className="grid gap-3 sm:grid-cols-[1fr_1fr_auto] sm:items-end">
            <Field label="Канистра, с" hint="Одна цена: розлив за литр считается от неё">
              <input inputMode="decimal" value={prices.canister} onChange={(e) => setPrices({ canister: e.target.value })} />
            </Field>
            <div className="pb-2 text-sm text-slate-600" data-testid="oil-pour-derived">
              {newPour !== null ? `на розлив: ${formatSom(newPour)} за л` : ''}
            </div>
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
          <Table head={owner ? ['№', 'Когда', 'Откуда', 'Куда', 'Сколько', 'Стоимость', 'Кто', ''] : ['№', 'Когда', 'Откуда', 'Куда', 'Сколько', 'Кто']}>
            {historyPager.shown.map((t) => (
              <tr key={t.id} className={t.reversed ? 'text-slate-400 line-through' : ''}>
                <td className="px-2 py-2 font-medium">
                  {t.number} <CorrectionBadges mark={marks.data?.get(t.id)} />
                </td>
                <td className="whitespace-nowrap px-2 py-2">{formatDateTime(t.created_at)}</td>
                <td className="px-2 py-2">{t.from_name}</td>
                <td className="px-2 py-2">{t.to_name}</td>
                <td className="whitespace-nowrap px-2 py-2">{formatLiters(t.qty_ml)}</td>
                {owner && <td className="whitespace-nowrap px-2 py-2">{formatSom(t.value_tyiyn ?? 0)}</td>}
                <td className="px-2 py-2">{t.user_name}</td>
                {owner && (
                <td className="px-2 py-2 text-right">
                  {t.reversed ? (
                    <span className="no-underline">отменён</span>
                  ) : (
                    <span className="inline-flex gap-1">
                      <FixButton
                        small
                        onClick={() => {
                          setFixing(t)
                          window.scrollTo({ top: 0, behavior: 'smooth' })
                        }}
                      />
                      <Button variant="secondary" className="px-2 py-1 text-xs" onClick={() => setReversing({ row: t, comment: '', opId: newOpId() })}>
                        Отменить
                      </Button>
                    </span>
                  )}
                </td>
                )}
              </tr>
            ))}
          </Table>
        )}
        <Pager page={historyPager.page} size={historyPager.size} total={historyPager.total} onPage={historyPager.setPage} onSize={historyPager.setSize} label="Страницы переливов" />
      </Card>
      {reversing && (
        <Modal title={`Отменить перелив № ${reversing.row.number}`} onClose={() => setReversing(null)}>
          <div className="flex flex-col gap-3">
            <div className="text-sm text-slate-600">
              {formatLiters(reversing.row.qty_ml)} вернутся из «{reversing.row.to_name}» в «{reversing.row.from_name}» той же стоимостью{' '}
              {formatSom(reversing.row.value_tyiyn ?? 0)}. Если перелитое уже продано, отменить нельзя — тогда перелейте обратно.
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
                    marks.reload()
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
