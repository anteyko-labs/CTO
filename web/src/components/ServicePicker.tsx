import { useState } from 'react'
import { get, noServerAnswer, post } from '../lib/api'
import { readSnapshot } from '../lib/offline'
import { useUser } from '../lib/auth'
import { formatSom, parseSom } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import { servicePriceText } from '../lib/serviceSale'
import type { Service } from '../lib/types'
import { Button, Empty, ErrorBox, Field, Loading, Missing, Modal } from './ui'

/**
 * Услуги в кассе: все заведённые работы плитками и новая услуга прямо отсюда (ADR-043).
 * Ставку мастера задаёт владелец, администратор создаёт услугу без неё. «В сервис» открывает
 * этот же выбор как «Вид замены» (`kinds`): крупные кнопки с ценой клиенту — или «бесплатно —
 * масло наше», если в чеке наше масло, — и ставкой мастеру (ADR-096).
 */
export function ServicePicker({
  onPick,
  onClose,
  onBatteryIntake,
  onBatterySale,
  ourOil = false,
  kinds = false,
}: {
  onPick: (s: Service) => void
  onClose: () => void
  onBatteryIntake?: () => void
  onBatterySale?: () => void
  /** В чеке есть наше масло: услуги «бесплатно с нашим маслом» стоят 0. */
  ourOil?: boolean
  /** Выбор вида замены для чека «В сервис». */
  kinds?: boolean
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

        {!form && kinds && <div className="text-sm text-slate-600">Без вида замены чек «В сервис» не провести. Можно выбрать несколько.</div>}
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
          <div className={`grid gap-2 ${kinds ? 'grid-cols-1 sm:grid-cols-2' : 'grid-cols-2 sm:grid-cols-3'}`} data-testid="service-choices">
            {shown.map((s) => (
              <button
                key={s.id}
                type="button"
                className={`flex flex-col justify-between rounded-lg border border-slate-200 bg-white text-left shadow-sm transition hover:border-sky-400 hover:bg-sky-50 active:bg-sky-100 ${
                  kinds ? 'min-h-[96px] p-4' : 'min-h-[76px] p-3'
                }`}
                onClick={() => onPick(s)}
              >
                <span className={`leading-snug font-medium ${kinds ? 'text-lg' : ''}`}>{s.name}</span>
                <span className="mt-1 flex items-baseline justify-between gap-2">
                  <span className={`font-semibold text-sky-700 ${kinds ? 'text-lg' : 'text-base'}`}>{servicePriceText(s, ourOil, formatSom)}</span>
                  {(kinds || s.master_fee_tyiyn > 0) && (
                    <span className={kinds ? 'text-base text-slate-600' : 'text-xs text-slate-500'}>мастеру {formatSom(s.master_fee_tyiyn)}</span>
                  )}
                </span>
              </button>
            ))}
          </div>
        )}
        <ErrorBox error={list.error} />
      </div>
    </Modal>
  )
}
