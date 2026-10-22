import { useState } from 'react'
import { get, noServerAnswer, post } from '../lib/api'
import { readSnapshot } from '../lib/offline'
import { useUser } from '../lib/auth'
import { formatSom, parseSom } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import { servicePriceText, type OilEnough } from '../lib/serviceSale'
import type { Service } from '../lib/types'
import { Button, Empty, ErrorBox, Field, Loading, Missing, Modal } from './ui'

const NO_OIL: OilEnough = new Set<string>()

/**
 * Услуги в кассе: все заведённые работы плитками и новая услуга прямо отсюда (ADR-043).
 * Ставку мастера задаёт владелец, администратор создаёт услугу без неё. «В сервис» открывает
 * этот же выбор как «Вид замены» (`kinds`): крупные кнопки с ценой клиенту — или «бесплатно —
 * масло наше», если в чеке наше масло, — и ставкой мастеру (ADR-096). Видов можно отметить
 * несколько: окно закрывает «Готово» (`onDone`), список прокручивается внутри окна.
 */
export function ServicePicker({
  onPick,
  onClose,
  onBatteryIntake,
  onBatterySale,
  ourOil = NO_OIL,
  kinds = false,
  selected = [],
  onDone,
}: {
  onPick: (s: Service) => void
  onClose: () => void
  onBatteryIntake?: () => void
  onBatterySale?: () => void
  /** Категории нашего масла, которого в чеке не меньше канистры: услуги «бесплатно с нашим
   *  маслом» этой категории стоят 0 (ADR-104). */
  ourOil?: OilEnough
  /** Выбор вида замены для чека «В сервис». */
  kinds?: boolean
  /** Виды замены, уже стоящие в чеке: отмечены сразу. */
  selected?: string[]
  /** «Готово» в выборе видов замены: все отмеченные. */
  onDone?: (picked: Service[]) => void
}) {
  const owner = useUser().role === 'owner'
  // Без связи — услуги из снимка кассы: работа уже сделана, чек пробивается (SPEC-09).
  const list = useLoad(
    () =>
      get<Service[]>('/services').catch(async (e: unknown) => {
        const snap = noServerAnswer(e) ? await readSnapshot() : null
        if (!snap?.services) throw e
        return snap.services
      }),
    [],
  )
  const [query, setQuery] = useState('')
  const [form, setForm] = useState<{ name: string; price: string; fee: string; free: boolean } | null>(null)
  const { busy, error, run } = useAction()
  const multi = kinds && Boolean(onDone)
  const [picked, setPicked] = useState<string[]>(selected)
  const toggle = (id: string) => setPicked((ids) => (ids.includes(id) ? ids.filter((x) => x !== id) : [...ids, id]))

  // Виды замены — сверху: их выбирают в каждом чеке «В сервис».
  const active = (list.data ?? []).filter((s) => s.active).sort((a, b) => Number(Boolean(b.free_with_oil)) - Number(Boolean(a.free_with_oil)))
  const q = query.trim().toLowerCase()
  const shown = q ? active.filter((s) => s.name.toLowerCase().includes(q)) : active

  const notFilled = form
    ? missingWithFocus(
        [Boolean(form.name.trim()), 'название', '#service-name'],
        [parseSom(form.price) !== null, 'цену', '#service-price'],
        [!owner || !form.fee.trim() || parseSom(form.fee) !== null, 'ставку мастера числом', '#service-fee'],
      )
    : []

  const create = () =>
    void run(async () => {
      if (!form) return
      const created = await post<Service>('/services', {
        name: form.name.trim(),
        price_tyiyn: parseSom(form.price) ?? 0,
        master_fee_tyiyn: owner && form.fee.trim() ? (parseSom(form.fee) ?? 0) : 0,
        free_with_oil: owner && form.free,
      })
      onPick(created)
    })

  return (
    <Modal title={kinds ? 'Вид замены' : 'Услуги'} onClose={onClose} wide>
      <div className="flex flex-col gap-3">
        {!form && (
          <div className="flex flex-wrap gap-2">
            <input
              autoFocus
              className="min-w-0 flex-1"
              placeholder="Найти услугу"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
            <Button variant="secondary" onClick={() => setForm({ name: query.trim(), price: '', fee: '', free: false })}>
              + Новая услуга
            </Button>
          </div>
        )}

        {!form && kinds && <div className="text-sm text-slate-600">Без вида замены чек «В сервис» не провести. Можно отметить несколько — затем «Готово».</div>}
        {!form && !kinds && (onBatteryIntake || onBatterySale) && (
          <div className="grid grid-cols-2 gap-2">
            {onBatteryIntake && (
              <button
                type="button"
                className="flex min-h-[64px] flex-col justify-between rounded-lg border border-emerald-300 bg-emerald-50 p-3 text-left hover:border-emerald-500"
                onClick={onBatteryIntake}
              >
                <span className="font-medium">Приём аккумуляторов</span>
                <span className="text-xs text-emerald-800">по весу, деньги из кассы</span>
              </button>
            )}
            {onBatterySale && (
              <button
                type="button"
                className="flex min-h-[64px] flex-col justify-between rounded-lg border border-amber-300 bg-amber-50 p-3 text-left hover:border-amber-500"
                onClick={onBatterySale}
              >
                <span className="font-medium">Аккумуляторы на вес</span>
                <span className="text-xs text-amber-800">продажа по весу, не дешевле средней за кг</span>
              </button>
            )}
          </div>
        )}
        {form ? (
          <div className="flex flex-col gap-3">
            <Field label="Название" required>
              <input id="service-name" autoFocus value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="Замена масла (масло клиента)" />
            </Field>
            <div className="grid grid-cols-2 gap-3">
              <Field label="Цена клиенту, с" required>
                <input id="service-price" inputMode="decimal" value={form.price} onChange={(e) => setForm({ ...form, price: e.target.value })} />
              </Field>
              <Field label="Мастеру, с" hint={owner ? 'Из цены — мастеру, остальное — в кассу' : 'Ставку мастера назначит владелец'}>
                <input id="service-fee" inputMode="decimal" disabled={!owner} value={form.fee} onChange={(e) => setForm({ ...form, fee: e.target.value })} />
              </Field>
            </div>
            {owner && (
              <label className="flex items-center gap-2 text-sm">
                <input type="checkbox" className="h-4 w-4" checked={form.free} onChange={(e) => setForm({ ...form, free: e.target.checked })} />
                Бесплатно с нашим маслом
              </label>
            )}
            {owner && parseSom(form.price) !== null && parseSom(form.fee) !== null && (
              <div className="text-sm text-slate-600">
                В кассу останется {formatSom((parseSom(form.price) ?? 0) - (parseSom(form.fee) ?? 0))}
              </div>
            )}
            <Missing items={notFilled} />
            <ErrorBox error={error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setForm(null)}>
                Назад
              </Button>
              <Button disabled={busy || notFilled.length > 0} onClick={create}>
                Создать и добавить в чек
              </Button>
            </div>
          </div>
        ) : list.loading && !list.data ? (
          <Loading />
        ) : shown.length === 0 ? (
          <Empty>{q ? 'Такой услуги нет — создайте её кнопкой «Новая услуга».' : 'Услуг пока нет. Создайте первую — она сразу встанет в чек.'}</Empty>
        ) : (
          // Список прокручивается внутри окна: окно помещается в экран 768 точек по высоте.
          <div
            className={`grid max-h-[calc(100dvh-21rem)] gap-2 overflow-y-auto overscroll-contain p-0.5 ${kinds ? 'grid-cols-1 sm:grid-cols-2' : 'grid-cols-2 sm:grid-cols-3'}`}
            data-testid="service-choices"
          >
            {shown.map((s) => {
              const on = multi && picked.includes(s.id)
              return (
              <button
                key={s.id}
                type="button"
                aria-pressed={multi ? on : undefined}
                className={`flex flex-col justify-between rounded-lg border text-left shadow-sm transition hover:border-sky-400 active:bg-sky-100 ${
                  on ? 'border-sky-600 bg-sky-50 ring-2 ring-sky-600' : 'border-slate-200 bg-white hover:bg-sky-50'
                } ${kinds ? 'min-h-[84px] p-3' : 'min-h-[76px] p-3'}`}
                onClick={() => (multi ? toggle(s.id) : onPick(s))}
              >
                <span className={`flex items-start gap-2 leading-snug font-medium ${kinds ? 'text-lg' : ''}`}>
                  {multi && (
                    <span
                      aria-hidden="true"
                      className={`mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded border-2 text-base ${on ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-400 bg-white'}`}
                    >
                      {on ? '✓' : ''}
                    </span>
                  )}
                  {s.name}
                </span>
                <span className="mt-1 flex items-baseline justify-between gap-2">
                  <span className={`font-semibold text-sky-700 ${kinds ? 'text-lg' : 'text-base'}`}>{servicePriceText(s, ourOil, formatSom)}</span>
                  {(kinds || s.master_fee_tyiyn > 0) && (
                    <span className={kinds ? 'text-base text-slate-600' : 'text-xs text-slate-500'}>мастеру {formatSom(s.master_fee_tyiyn)}</span>
                  )}
                </span>
              </button>
              )
            })}
          </div>
        )}
        <ErrorBox error={list.error} />
        {multi && !form && (
          <div className="flex items-center justify-between gap-3 border-t border-slate-200 pt-3">
            <span className="text-sm text-slate-600">{picked.length ? `Отмечено: ${picked.length}` : 'Ничего не отмечено'}</span>
            <Button
              className="min-h-[56px]! px-8 text-lg!"
              onClick={() => onDone?.((list.data ?? []).filter((s) => picked.includes(s.id)))}
            >
              Готово
            </Button>
          </div>
        )}
      </div>
    </Modal>
  )
}
