// «Занести старый долг» из тетради (SPEC-38, ADR-133): клиент из списка или новый, сумма, дата долга
// и комментарий. Запись «долг» ложится в день долга — от него срок оплаты и просрочка. Только при
// связи с сервером (§6). То же окно — «Исправить» записи старого долга владельцем (ADR-085).
import { useState } from 'react'
import { ApiError, get, newOpId, post, qs } from '../lib/api'
import { formatSom, parseSom, somInput, todayBishkek } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useDebounced, useLoad } from '../lib/hooks'
import { useCan } from '../lib/permissions'
import { earliestOldDebtDate, OLD_DEBT_DUPLICATE, oldDebtDateError, oldDebtPreview, olderThanYear, type OldDebtDoc } from '../lib/oldDebt'
import type { Party } from '../lib/types'
import { balanceText } from './ClientPicker'
import { CorrectionBanner, useCorrection } from './Correction'
import { Button, ErrorBox, Field, Loading, Missing, Modal, toast } from './ui'

interface OldDebtOut {
  party_id: string
  ledger_id: string
  balance_tyiyn: number
  due_date: string | null
  overdue_days: number
}

const EMPTY_NEW = { kind: 'person' as Party['kind'], first: '', last: '', name: '', phone: '', inn: '' }

/** Поиск клиента по имени, телефону, ПИН/ИНН — как на кассе; без связи окно не работает (§6). */
function useClientSearch(text: string) {
  const query = useDebounced(text.trim(), 250)
  return useLoad(
    () => (query.length < 2 ? Promise.resolve<Party[]>([]) : get<Party[]>(`/parties${qs({ role: 'customer', q: query, limit: 8 })}`)),
    [query],
  )
}

export function OldDebtModal({
  party: initial = null,
  fixId,
  onClose,
  onSaved,
}: {
  /** Клиент уже выбран — из карточки клиента. */
  party?: Party | null
  /** «Исправить» проведённый старый долг: id записи долга; окно заполняется ею. */
  fixId?: string
  onClose: () => void
  /** Запись проведена: обновить список или карточку; окно закрывается само, кроме «Занести и ещё». */
  onSaved: () => void
}) {
  const today = todayBishkek()
  const mayCreate = useCan()('clients')
  const [party, setParty] = useState<Party | null>(initial)
  const [search, setSearch] = useState('')
  const [creating, setCreating] = useState(false)
  const [np, setNp] = useState(EMPTY_NEW)
  const [sum, setSum] = useState('')
  const [date, setDate] = useState('')
  const [comment, setComment] = useState('')
  // Повтор: «Такой долг уже занесён 28.10 (Эрбол). Занести ещё раз?» — второе подтверждение.
  const [duplicate, setDuplicate] = useState<{ text: string; more: boolean } | null>(null)
  const [opId, setOpId] = useState(newOpId)
  const { busy, error, setError, run } = useAction()
  const found = useClientSearch(creating ? '' : search)
  // Похожие на нового клиента по фамилии, названию и телефону — чтобы не завести дубль.
  const similarText = np.kind === 'company' ? np.name : np.last
  const similarByName = useClientSearch(creating ? similarText : '')
  const similarByPhone = useClientSearch(creating ? np.phone : '')
  const similar = [...(similarByName.data ?? []), ...(similarByPhone.data ?? [])].filter((p, i, all) => all.findIndex((x) => x.id === p.id) === i)

  // Исправление: поля — из исходной записи, как только она пришла с сервера.
  const correction = useCorrection<OldDebtDoc>('ledger', fixId ?? null)
  const fixDraft = fixId ? correction.draft.data : null
  const [filled, setFilled] = useState(false)
  if (fixDraft && !filled) {
    setSum(somInput(fixDraft.doc.amount_tyiyn))
    setDate(fixDraft.doc.date)
    setComment(fixDraft.doc.comment)
    setFilled(true)
  }

  const amount = parseSom(sum)
  const dateError = date ? oldDebtDateError(date, today) : null
  const company = np.kind === 'company'
  const notFilled = missingWithFocus(
    [Boolean(party) || creating, 'клиента', '#old-debt-client'],
    [!creating || company || Boolean(np.last.trim()), 'фамилию', '#old-debt-last'],
    [!creating || company || Boolean(np.first.trim()), 'имя', '#old-debt-first'],
    [!creating || !company || Boolean(np.name.trim()), 'название фирмы', '#old-debt-name'],
    [amount !== null && amount > 0, 'сумму', '#old-debt-sum'],
    [Boolean(date) && !dateError, 'дату долга', '#old-debt-date'],
  )
  // У новой фирмы срок оплаты 14 дней по умолчанию (ADR-083), у физлица срока нет.
  const dueDays = creating ? (company ? 14 : null) : (party?.due_days ?? null)
  const preview = oldDebtPreview(creating ? 0 : (party?.balance_tyiyn ?? 0), amount, date, dueDays, today)

  const pick = (p: Party) => {
    setParty(p)
    setSearch('')
    setCreating(false)
    setDuplicate(null)
  }

  const body = (confirm: boolean) => ({
    op_id: opId,
    ...(creating
      ? {
          new_party: {
            kind: np.kind,
            first_name: company ? '' : np.first.trim(),
            last_name: company ? '' : np.last.trim(),
            name: company ? np.name.trim() : '',
            phone: np.phone.trim(),
            inn: np.inn.trim(),
          },
        }
      : { party_id: party?.id }),
    amount_tyiyn: amount,
    date,
    comment: comment.trim(),
    confirm_duplicate: confirm,
  })

  const save = (more: boolean, confirm = false) =>
    void run(async () => {
      if (notFilled.length > 0 || amount === null) return
      let out: OldDebtOut
      try {
        out = await post<OldDebtOut>('/debts/old', body(confirm))
      } catch (e) {
        if (e instanceof ApiError && e.status === 409 && e.message.startsWith(OLD_DEBT_DUPLICATE)) {
          setDuplicate({ text: e.message, more })
          return
        }
        throw e
      }
      const who = creating ? (company ? np.name.trim() : `${np.last.trim()} ${np.first.trim()}`) : (party?.name ?? '')
      toast(`Старый долг занесён: ${who} — ${formatSom(amount)}. Долг клиента — ${formatSom(out.balance_tyiyn)}`)
      setDuplicate(null)
      onSaved()
      if (!more) {
        onClose()
        return
      }
      // «Занести и ещё»: окно остаётся, дата сохраняется, клиент и сумма очищаются (§1).
      setOpId(newOpId())
      setParty(null)
      setCreating(false)
      setNp(EMPTY_NEW)
      setSum('')
      setComment('')
      setError(null)
    })

  const saveFix = async () => {
    if (!fixDraft || !party || amount === null || amount <= 0 || dateError || !date) return
    const doc: OldDebtDoc = { ...fixDraft.doc, party_id: party.id, amount_tyiyn: amount, date, comment: comment.trim() }
    const c = await correction.save(doc)
    if (!c) return
    toast(`Исправлено: старый долг ${formatSom(amount)}`)
    onSaved()
    onClose()
  }

  return (
    <Modal title={fixId ? 'Исправить старый долг' : 'Занести старый долг'} onClose={onClose}>
      <div className="flex flex-col gap-3" data-testid="old-debt-modal">
        {fixDraft && <CorrectionBanner label={fixDraft.kind_label} number={fixDraft.number} comment={correction.comment} onComment={correction.setComment} />}
        {fixId && !filled && correction.draft.loading && <Loading />}

        <div className="flex flex-col gap-1 text-sm">
          <span className="font-medium text-slate-700">
            Клиент<span className="ml-0.5 text-rose-600">*</span>
          </span>
          {party ? (
            <div className="flex items-center justify-between gap-2 rounded-md border border-slate-300 px-3 py-2" data-testid="old-debt-party">
              <span>
                <span className="font-medium">{party.name}</span>
                <span className="ml-2 text-slate-500">{balanceText(party.balance_tyiyn)}</span>
              </span>
              {(!initial || fixId) && (
                <Button variant="ghost" className="px-2 py-1 text-xs" onClick={() => setParty(null)}>
                  Сменить
                </Button>
              )}
            </div>
          ) : creating ? (
            <div className="flex flex-col gap-3 rounded-md border border-slate-200 p-3">
              <div className="flex items-center justify-between gap-2">
                <div className="flex w-fit overflow-hidden rounded-md border border-slate-300 text-sm">
                  {(
                    [
                      ['person', 'Физлицо'],
                      ['company', 'Фирма'],
                    ] as const
                  ).map(([k, label]) => (
                    <button
                      key={k}
                      type="button"
                      className={`px-3 py-2 ${np.kind === k ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
                      onClick={() => setNp({ ...np, kind: k })}
                    >
                      {label}
                    </button>
                  ))}
                </div>
                <Button variant="ghost" className="px-2 py-1 text-xs" onClick={() => setCreating(false)}>
                  Найти в списке
                </Button>
              </div>
              {company ? (
                <Field label="Название фирмы" required>
                  <input id="old-debt-name" autoFocus value={np.name} onChange={(e) => setNp({ ...np, name: e.target.value })} />
                </Field>
              ) : (
                <div className="grid gap-3 sm:grid-cols-2">
                  <Field label="Фамилия" required>
                    <input id="old-debt-last" autoFocus value={np.last} onChange={(e) => setNp({ ...np, last: e.target.value })} />
                  </Field>
                  <Field label="Имя" required>
                    <input id="old-debt-first" value={np.first} onChange={(e) => setNp({ ...np, first: e.target.value })} />
                  </Field>
                </div>
              )}
              <div className="grid gap-3 sm:grid-cols-2">
                <Field label="Телефон">
                  <input id="old-debt-phone" type="tel" value={np.phone} onChange={(e) => setNp({ ...np, phone: e.target.value })} />
                </Field>
                <Field label={company ? 'ИНН' : 'ПИН'} hint="по желанию: касса спросит при следующей продаже в долг">
                  <input id="old-debt-inn" inputMode="numeric" value={np.inn} onChange={(e) => setNp({ ...np, inn: e.target.value })} />
                </Field>
              </div>
              {similar.length > 0 && (
                <div className="rounded-md bg-amber-50 p-2 text-xs text-amber-900" data-testid="old-debt-similar">
                  <div className="mb-1">Похожие клиенты уже есть — может, это он?</div>
                  <div className="flex flex-wrap gap-1">
                    {similar.slice(0, 5).map((p) => (
                      <Button key={p.id} variant="secondary" className="min-h-0 px-2 py-1 text-xs" onClick={() => pick(p)}>
                        {p.name}
                        {p.phone && ` · ${p.phone}`}
                      </Button>
                    ))}
                  </div>
                </div>
              )}
            </div>
          ) : (
            <>
              <input id="old-debt-client" aria-label="Клиент" autoFocus placeholder="Имя, телефон, ПИН или ИНН" value={search} onChange={(e) => setSearch(e.target.value)} />
              <ErrorBox error={found.error} />
              {(found.data ?? []).length > 0 && (
                <ul className="divide-y divide-slate-100 rounded-md border border-slate-200">
                  {(found.data ?? []).map((p) => (
                    <li key={p.id}>
                      <button type="button" className="flex w-full items-center justify-between gap-2 px-3 py-2 text-left hover:bg-slate-50" onClick={() => pick(p)}>
                        <span className="font-medium">{p.name}</span>
                        <span className="text-xs text-slate-500">
                          {[p.phone, p.inn, balanceText(p.balance_tyiyn)].filter(Boolean).join(' · ')}
                        </span>
                      </button>
                    </li>
                  ))}
                </ul>
              )}
              {search.trim().length >= 2 && !found.loading && (found.data ?? []).length === 0 && <span className="text-xs text-slate-500">Не нашли</span>}
              {mayCreate && !fixId && (
                <Button
                  variant="secondary"
                  className="w-fit"
                  onClick={() => {
                    setCreating(true)
                    setNp({ ...EMPTY_NEW, last: search.trim() })
                  }}
                >
                  + Новый клиент
                </Button>
              )}
            </>
          )}
        </div>

        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Сумма, с" required>
            <input id="old-debt-sum" inputMode="decimal" value={sum} onChange={(e) => {
                setSum(e.target.value)
                setDuplicate(null)
              }} />
          </Field>
          <Field label="Дата долга" required hint="Когда взял: от неё считаются срок оплаты и просрочка" error={dateError}>
            <input
              id="old-debt-date"
              type="date"
              min={earliestOldDebtDate(today)}
              max={today}
              value={date}
              onChange={(e) => {
                setDate(e.target.value)
                setDuplicate(null)
              }}
            />
          </Field>
        </div>
        <Field label="Комментарий" hint="Что брал, страница тетради">
          <input id="old-debt-comment" maxLength={500} value={comment} onChange={(e) => setComment(e.target.value)} />
        </Field>

        {date && !dateError && olderThanYear(date, today) && (
          <div className="rounded-md bg-amber-50 px-3 py-2 text-sm text-amber-900">Долгу больше года — проверьте год</div>
        )}
        {preview.length > 0 && (
          <div className="flex flex-col gap-0.5 text-sm text-slate-600" data-testid="old-debt-preview">
            {preview.map((l) => (
              <div key={l}>{l}</div>
            ))}
          </div>
        )}

        {duplicate && (
          <div className="flex flex-col gap-2 rounded-md bg-amber-50 p-3 text-sm text-amber-900" data-testid="old-debt-duplicate">
            <div>{duplicate.text}</div>
            <div className="flex gap-2">
              <Button variant="secondary" onClick={() => setDuplicate(null)}>
                Не заносить
              </Button>
              <Button disabled={busy} onClick={() => save(duplicate.more, true)}>
                Занести ещё раз
              </Button>
            </div>
          </div>
        )}

        <ErrorBox error={fixId ? correction.error : error} />
        <div className="flex flex-wrap items-center justify-end gap-2">
          <Missing items={notFilled} className="mr-auto" />
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          {fixId ? (
            <Button
              disabled={correction.busy || !fixDraft || notFilled.length > 0 || !correction.comment.trim()}
              onClick={() => void saveFix()}
            >
              Сохранить исправление
            </Button>
          ) : (
            <>
              <Button variant="secondary" disabled={busy || notFilled.length > 0 || duplicate !== null} onClick={() => save(true)}>
                Занести и ещё
              </Button>
              <Button disabled={busy || notFilled.length > 0 || duplicate !== null} onClick={() => save(false)}>
                Занести
              </Button>
            </>
          )}
        </div>
      </div>
    </Modal>
  )
}
