import { useState } from 'react'
import { get, post } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatSom, parseSom } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import type { Service } from '../lib/types'
import { Button, Empty, ErrorBox, Field, Loading, Missing, Modal } from './ui'

/**
 * Услуги в кассе: все заведённые работы плитками и новая услуга прямо отсюда (ADR-043).
 * Ставку мастера задаёт владелец, администратор создаёт услугу без неё.
 */
export function ServicePicker({ onPick, onClose }: { onPick: (s: Service) => void; onClose: () => void }) {
  const owner = useUser().role === 'owner'
  const list = useLoad(() => get<Service[]>('/services'), [])
  const [query, setQuery] = useState('')
  const [form, setForm] = useState<{ name: string; price: string; fee: string } | null>(null)
  const { busy, error, run } = useAction()

  const active = (list.data ?? []).filter((s) => s.active)
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
      })
      onPick(created)
    })

  return (
    <Modal title="Услуги" onClose={onClose} wide>
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
            <Button variant="secondary" onClick={() => setForm({ name: query.trim(), price: '', fee: '' })}>
              + Новая услуга
            </Button>
          </div>
        )}

        {form ? (
          <div className="flex flex-col gap-3">
            <Field label="Название" required>
              <input id="service-name" autoFocus value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="Замена масла (масло клиента)" />
            </Field>
            <div className="grid grid-cols-2 gap-3">
              <Field label="Цена, с" required>
                <input id="service-price" inputMode="decimal" value={form.price} onChange={(e) => setForm({ ...form, price: e.target.value })} />
              </Field>
              <Field label="Мастеру, с" hint={owner ? 'Из цены — мастеру, остальное — в кассу' : 'Ставку мастера назначит владелец'}>
                <input id="service-fee" inputMode="decimal" disabled={!owner} value={form.fee} onChange={(e) => setForm({ ...form, fee: e.target.value })} />
              </Field>
            </div>
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
          <div className="grid grid-cols-2 gap-2 sm:grid-cols-3">
            {shown.map((s) => (
              <button
                key={s.id}
                type="button"
                className="flex min-h-[76px] flex-col justify-between rounded-lg border border-slate-200 bg-white p-3 text-left shadow-sm transition hover:border-sky-400 hover:bg-sky-50"
                onClick={() => onPick(s)}
              >
                <span className="font-medium leading-snug">{s.name}</span>
                <span className="mt-1 flex items-baseline justify-between gap-2">
                  <span className="text-base font-semibold text-sky-700">{formatSom(s.price_tyiyn)}</span>
                  {s.master_fee_tyiyn > 0 && <span className="text-xs text-slate-500">мастеру {formatSom(s.master_fee_tyiyn)}</span>}
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
