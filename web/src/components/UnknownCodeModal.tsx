// Окно незнакомого штрихкода: добавить код к заведённому товару или завести новый (ADR-026,
// задача 005 Н3).
import { useRef, useState } from 'react'
import { get, qs } from '../lib/api'
import { formatSom } from '../lib/format'
import { useAction, useDebounced, useLoad } from '../lib/hooks'
import { linkCode } from '../lib/linkCode'
import type { Product } from '../lib/types'
import { Button, Card, Empty, ErrorBox, Field, Loading, Modal } from './ui'
import { stockText } from './ProductPicker'
import { perWord } from '../lib/units'

/** Цена продажи в строке найденного: за канистру, кг, м или штуку (себестоимости нет, I13). */
function priceText(p: Product): string {
  const per = p.unit === 'piece' ? '' : ` ${perWord(p.unit, p.container_ml)}`
  return `${formatSom(p.sale_price_tyiyn)}${per}`
}

/**
 * Незнакомый штрихкод: его либо добавляют к уже заведённому товару, либо заводят новый — сразу
 * две большие кнопки. Привязка — главный способ не разводить карточки-дубли (ADR-026).
 * `onCreateNew` нет (касса с правом «Привязывать коды на кассе» без «Товаров») — только привязка.
 * `pickLabel` — кнопка «продолжить с товаром, у которого код уже есть» (на кассе — «Положить в чек»).
 */
export function UnknownCodeModal({
  code,
  onClose,
  onLinked,
  onCreateNew,
  pickLabel = 'Продолжить с ним',
}: {
  code: string
  onClose: () => void
  onLinked: (p: Product) => void
  onCreateNew?: () => void
  pickLabel?: string
}) {
  const [attaching, setAttaching] = useState(!onCreateNew)
  const [q, setQ] = useState('')
  const query = useDebounced(q.trim(), 200)
  const found = useLoad(
    () => (!attaching || query.length < 2 ? Promise.resolve<Product[]>([]) : get<Product[]>(`/products${qs({ q: query, limit: 10 })}`)),
    [query, attaching],
  )
  const link = useAction()
  const [taken, setTaken] = useState<Product | null>(null)
  const [measure, setMeasure] = useState<{ p: Product; message: string } | null>(null)
  const search = useRef<HTMLInputElement>(null)

  const attach = (p: Product, measureOk = false) =>
    link.run(async () => {
      setTaken(null)
      setMeasure(null)
      const r = await linkCode(p.id, code, measureOk)
      if (r.kind === 'linked') onLinked(r.product)
      else if (r.kind === 'taken') setTaken(r.product)
      else setMeasure({ p, message: r.message })
    })

  const big = 'min-h-[64px]! flex-1 text-base!'

  return (
    <Modal title={`Код ${code} не найден`} onClose={onClose}>
      <div className="flex flex-col gap-3">
        <p className="text-sm text-slate-600">Если товар уже заведён, добавьте код к нему — так в справочнике не появится второй такой же товар.</p>
        <div className="flex flex-col gap-2 sm:flex-row">
          <Button
            className={big}
            variant={attaching ? 'primary' : 'secondary'}
            data-testid="unknown-attach"
            onClick={() => {
              setAttaching(true)
              search.current?.focus()
            }}
          >
            Добавить к существующему товару
          </Button>
          {onCreateNew && (
            <Button className={big} variant="secondary" onClick={onCreateNew}>
              Новый товар
            </Button>
          )}
        </div>
        {taken && (
          <Card className="flex flex-wrap items-center justify-between gap-3 border-amber-300 bg-amber-50 py-2" data-testid="unknown-taken">
            <div>
              Этот код уже у «{taken.name}»
              <div className="text-xs text-slate-500">
                {priceText(taken)} · {stockText(taken)}
              </div>
            </div>
            <Button onClick={() => onLinked(taken)}>{pickLabel}</Button>
          </Card>
        )}
        {measure && (
          <Card className="flex flex-col gap-2 border-amber-300 bg-amber-50 py-2" data-testid="unknown-measure">
            <div>{measure.message}</div>
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setMeasure(null)}>
                Нет
              </Button>
              <Button disabled={link.busy} onClick={() => void attach(measure.p, true)}>
                Да, привязать к «{measure.p.name}»
              </Button>
            </div>
          </Card>
        )}
        {attaching && (
          <>
            <Field label="Найти товар">
              <input ref={search} autoFocus placeholder="Название, бренд, артикул или другой код" value={q} onChange={(e) => setQ(e.target.value)} />
            </Field>
            <ErrorBox error={found.error ?? link.error} />
            {found.loading && !found.data ? (
              <Loading />
            ) : query.length < 2 ? (
              <Empty>Введите хотя бы две буквы или цифры</Empty>
            ) : (found.data ?? []).length === 0 ? (
              <Empty>Ничего не нашлось</Empty>
            ) : (
              <ul className="flex flex-col gap-2">
                {(found.data ?? []).map((p) => (
                  <li key={p.id}>
                    <Card className="flex items-center justify-between gap-3 py-2">
                      <div className="min-w-0">
                        <div className="font-medium">{p.name}</div>
                        <div className="text-xs text-slate-500">
                          {p.brand || '—'} · {p.article || 'без артикула'} · {priceText(p)} · {stockText(p)}
                        </div>
                        {p.barcodes.length > 0 && <div className="truncate text-xs text-slate-500">коды: {p.barcodes.join(', ')}</div>}
                      </div>
                      <Button variant="secondary" disabled={link.busy} onClick={() => void attach(p)}>
                        Привязать код
                      </Button>
                    </Card>
                  </li>
                ))}
              </ul>
            )}
          </>
        )}
        <div className="flex justify-end gap-2 border-t border-slate-200 pt-3">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
        </div>
      </div>
    </Modal>
  )
}
