// «Занести старый долг» поставщику (задача 003 Н15, ADR-163): наш долг из тетради — поставщик,
// сумма, дата долга, номер его накладной и комментарий. Запись ложится в день долга и гасится
// оплатой поставщику, как накладная (самые старые первыми). Только при связи с сервером. То же окно —
// «Исправить» записи владельцем (ADR-085).
import { useState } from 'react'
import { ApiError, get, newOpId, post, qs } from '../lib/api'
import { balanceText } from '../lib/defects'
import { formatSom, parseSom, somInput, todayBishkek } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useDebounced, useLoad } from '../lib/hooks'
import {
  earliestOldDebtDate,
  OLD_DEBT_DUPLICATE,
  oldDebtDateError,
  olderThanYear,
  SUPPLIER_DOC_MAX,
  supplierDocError,
  supplierOldDebtPreview,
  type SupplierOldDebtDoc,
} from '../lib/oldDebt'
import type { Party } from '../lib/types'
import { CorrectionBanner, useCorrection } from './Correction'
import { Button, ErrorBox, Field, Loading, Missing, Modal, toast } from './ui'

interface SupplierOldDebtOut {
  party_id: string
  ledger_id: string
  balance_tyiyn: number
}

/** Баланс поставщика в филиале: меньше нуля — мы должны (SPEC-33 §2). */
const branchBalance = (p: Party) => p.branch_balance_tyiyn ?? p.balance_tyiyn

export function SupplierOldDebtModal({
  party: initial = null,
  fixId,
  onClose,
  onSaved,
}: {
  /** Поставщик уже выбран — из его карточки. */
  party?: Party | null
  /** «Исправить» проведённый старый долг: id записи долга. */
  fixId?: string
  onClose: () => void
  /** Запись проведена: обновить список или карточку; окно закрывается само, кроме «Занести и ещё». */
  onSaved: () => void
}) {
  const today = todayBishkek()
  const [party, setParty] = useState<Party | null>(initial)
  const [search, setSearch] = useState('')
  const [sum, setSum] = useState('')
  const [date, setDate] = useState('')
  const [doc, setDoc] = useState('')
  const [comment, setComment] = useState('')
  const [duplicate, setDuplicate] = useState<{ text: string; more: boolean } | null>(null)
  const [opId, setOpId] = useState(newOpId)
  const { busy, error, setError, run } = useAction()
  const query = useDebounced(search.trim(), 250)
  const found = useLoad(
    () => (party || query.length < 2 ? Promise.resolve<Party[]>([]) : get<Party[]>(`/parties${qs({ role: 'supplier', q: query, limit: 8 })}`)),
    [query, party],
  )

  const correction = useCorrection<SupplierOldDebtDoc>('ledger', fixId ?? null)
  const fixDraft = fixId ? correction.draft.data : null
  const [filled, setFilled] = useState(false)
  if (fixDraft && !filled) {
    setSum(somInput(fixDraft.doc.amount_tyiyn))
    setDate(fixDraft.doc.date)
    setDoc(fixDraft.doc.supplier_doc)
    setComment(fixDraft.doc.comment)
    setFilled(true)
  }

  const amount = parseSom(sum)
  const dateError = date ? oldDebtDateError(date, today) : null
  const docError = supplierDocError(doc)
  const notFilled = missingWithFocus(
    [Boolean(party), 'поставщика', '#supplier-debt-party'],
    [amount !== null && amount > 0, 'сумму', '#supplier-debt-sum'],
    [Boolean(date) && !dateError, 'дату долга', '#supplier-debt-date'],
  )
  const preview = supplierOldDebtPreview(party ? branchBalance(party) : 0, amount)

  const save = (more: boolean, confirm = false) =>
    void run(async () => {
      if (notFilled.length > 0 || amount === null || !party || docError) return
      let out: SupplierOldDebtOut
      try {
        out = await post<SupplierOldDebtOut>('/debts/old-supplier', {
          op_id: opId,
          party_id: party.id,
          amount_tyiyn: amount,
          date,
          supplier_doc: doc.trim(),
          comment: comment.trim(),
          confirm_duplicate: confirm,
        })
      } catch (e) {
        if (e instanceof ApiError && e.status === 409 && e.message.startsWith(OLD_DEBT_DUPLICATE)) {
          setDuplicate({ text: e.message, more })
          return
        }
        throw e
      }
      toast(`Старый долг поставщику занесён: ${party.name} — ${formatSom(amount)}. Мы должны — ${formatSom(Math.max(0, -out.balance_tyiyn))}`)
      setDuplicate(null)
      onSaved()
      if (!more) {
        onClose()
        return
      }
      // «Занести и ещё»: поставщик и дата остаются, сумма, накладная и комментарий очищаются.
      setOpId(newOpId())
      setParty({ ...party, balance_tyiyn: out.balance_tyiyn, branch_balance_tyiyn: out.balance_tyiyn })
      setSum('')
      setDoc('')
      setComment('')
      setError(null)
    })

  const saveFix = async () => {
    if (!fixDraft || !party || amount === null || amount <= 0 || dateError || !date || docError) return
    const next: SupplierOldDebtDoc = { ...fixDraft.doc, party_id: party.id, amount_tyiyn: amount, date, supplier_doc: doc.trim(), comment: comment.trim() }
    const c = await correction.save(next)
    if (!c) return
    toast(`Исправлено: старый долг поставщику ${formatSom(amount)}`)
    onSaved()
    onClose()
  }

  return (
    <Modal title={fixId ? 'Исправить старый долг поставщику' : 'Старый долг поставщику'} onClose={onClose}>
      <div className="flex flex-col gap-3" data-testid="supplier-old-debt-modal">
        {fixDraft && <CorrectionBanner label={fixDraft.kind_label} number={fixDraft.number} comment={correction.comment} onComment={correction.setComment} />}
        {fixId && !filled && correction.draft.loading && <Loading />}
        <p className="text-sm text-slate-600">
          Долг Автодома поставщику, оставшийся с тетради. Деньги и прибыль не меняются; оплата поставщику гасит его, как накладную, — самые старые долги первыми.
        </p>

        <div className="flex flex-col gap-1 text-sm">
          <span className="font-medium text-slate-700">
            Поставщик<span className="ml-0.5 text-rose-600">*</span>
          </span>
          {party ? (
            <div className="flex items-center justify-between gap-2 rounded-md border border-slate-300 px-3 py-2" data-testid="supplier-debt-party">
              <span>
                <span className="font-medium">{party.name}</span>
                <span className="ml-2 text-slate-500">{balanceText(branchBalance(party))}</span>
              </span>
              {(!initial || fixId) && (
                <Button variant="ghost" className="px-2 py-1 text-xs" onClick={() => setParty(null)}>
                  Сменить
                </Button>
              )}
            </div>
          ) : (
            <>
              <input id="supplier-debt-party" aria-label="Поставщик" autoFocus placeholder="Название поставщика" value={search} onChange={(e) => setSearch(e.target.value)} />
              <ErrorBox error={found.error} />
              {(found.data ?? []).length > 0 && (
                <ul className="divide-y divide-slate-100 rounded-md border border-slate-200">
                  {(found.data ?? []).map((p) => (
                    <li key={p.id}>
                      <button
                        type="button"
                        className="flex w-full items-center justify-between gap-2 px-3 py-2 text-left hover:bg-slate-50"
                        onClick={() => {
                          setParty(p)
                          setSearch('')
                          setDuplicate(null)
                        }}
                      >
                        <span className="font-medium">{p.name}</span>
                        <span className="text-xs text-slate-500">{[p.phone, balanceText(branchBalance(p))].filter(Boolean).join(' · ')}</span>
                      </button>
                    </li>
                  ))}
                </ul>
              )}
              {query.length >= 2 && !found.loading && (found.data ?? []).length === 0 && (
                <span className="text-xs text-slate-500">Не нашли. Нового поставщика заводят в «Поставщиках».</span>
              )}
            </>
          )}
        </div>

        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Сумма, с" required hint="Сколько мы должны">
            <input
              id="supplier-debt-sum"
              inputMode="decimal"
              value={sum}
              onChange={(e) => {
                setSum(e.target.value)
                setDuplicate(null)
              }}
            />
          </Field>
          <Field label="Дата долга" required hint="Когда получили товар: с неё долг в акте сверки" error={dateError}>
            <input
              id="supplier-debt-date"
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
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="№ накладной поставщика" hint="по желанию" error={docError}>
            <input id="supplier-debt-doc" maxLength={SUPPLIER_DOC_MAX} value={doc} onChange={(e) => setDoc(e.target.value)} />
          </Field>
          <Field label="Комментарий" hint="Что брали, страница тетради">
            <input id="supplier-debt-comment" maxLength={500} value={comment} onChange={(e) => setComment(e.target.value)} />
          </Field>
        </div>

        {date && !dateError && olderThanYear(date, today) && (
          <div className="rounded-md bg-amber-50 px-3 py-2 text-sm text-amber-900">Долгу больше года — проверьте год</div>
        )}
        {!fixId && preview.length > 0 && (
          <div className="text-sm text-slate-600" data-testid="supplier-debt-preview">
            {preview.map((l) => (
              <div key={l}>{l}</div>
            ))}
          </div>
        )}

        {duplicate && (
          <div className="flex flex-col gap-2 rounded-md bg-amber-50 p-3 text-sm text-amber-900" data-testid="supplier-debt-duplicate">
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
            <Button disabled={correction.busy || !fixDraft || notFilled.length > 0 || Boolean(docError) || !correction.comment.trim()} onClick={() => void saveFix()}>
              Сохранить исправление
            </Button>
          ) : (
            <>
              <Button variant="secondary" disabled={busy || notFilled.length > 0 || Boolean(docError) || duplicate !== null} onClick={() => save(true)}>
                Занести и ещё
              </Button>
              <Button disabled={busy || notFilled.length > 0 || Boolean(docError) || duplicate !== null} onClick={() => save(false)}>
                Занести
              </Button>
            </>
          )}
        </div>
      </div>
    </Modal>
  )
}
