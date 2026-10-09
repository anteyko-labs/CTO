// Скидка баллами на кассе: клиент из Телеграм-бота по номеру телефона (SPEC-19).
import { useState } from 'react'
import { get, qs } from '../lib/api'
import { formatSom, parseSom, somInput } from '../lib/format'
import { useAction } from '../lib/hooks'
import { Button, ErrorBox, Field, Modal } from './ui'

export interface BonusUse {
  party_id: string
  name: string
  balance_tyiyn: number
  amount_tyiyn: number
}

interface Found {
  party_id: string
  name: string
  phone: string
  balance_tyiyn: number
}

export function BonusRedeem({ total, onApply, onClose }: { total: number; onApply: (b: BonusUse) => void; onClose: () => void }) {
  const [phone, setPhone] = useState('')
  const [found, setFound] = useState<Found | null>(null)
  const [sum, setSum] = useState('')
  const act = useAction()
  const amount = sum.trim() ? parseSom(sum) : null
  const max = found ? Math.min(found.balance_tyiyn, total) : 0

  const find = () =>
    void act.run(async () => {
      const f = await get<Found>(`/loyalty/lookup${qs({ phone })}`)
      setFound(f)
      setSum(somInput(Math.max(0, Math.min(f.balance_tyiyn, total))))
    })

  return (
    <Modal title="Скидка баллами" onClose={onClose}>
      <div className="flex flex-col gap-3">
        <div className="flex items-end gap-2">
          <Field label="Номер телефона клиента" hint="Тот, которым клиент подключился к Телеграм-боту">
            <input
              autoFocus
              inputMode="tel"
              value={phone}
              onChange={(e) => {
                setPhone(e.target.value)
                setFound(null)
              }}
              onKeyDown={(e) => e.key === 'Enter' && find()}
              placeholder="0555 12 34 56"
            />
          </Field>
          <Button variant="secondary" disabled={act.busy || phone.replace(/\D/g, '').length < 9} onClick={find}>
            Найти
          </Button>
        </div>
        <ErrorBox error={act.error} />
        {found && (
          <>
            <div className="rounded-md bg-sky-50 px-3 py-2 text-sm">
              {found.name}: баллов <span className="font-semibold">{formatSom(found.balance_tyiyn).replace(/\s*с$/, '')}</span> (1 балл = 1 сом)
            </div>
            {max > 0 ? (
              <Field label="Списать, баллов" hint={`Не больше ${formatSom(max)}`}>
                <input inputMode="decimal" value={sum} onChange={(e) => setSum(e.target.value)} />
              </Field>
            ) : (
              <div className="text-sm text-slate-600">Списывать нечего{total <= 0 ? ': чек пуст' : ''}.</div>
            )}
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={onClose}>
                Отмена
              </Button>
              <Button
                disabled={amount === null || amount <= 0 || amount > max}
                onClick={() => amount !== null && onApply({ party_id: found.party_id, name: found.name, balance_tyiyn: found.balance_tyiyn, amount_tyiyn: amount })}
              >
                Списать
              </Button>
            </div>
          </>
        )}
      </div>
    </Modal>
  )
}
