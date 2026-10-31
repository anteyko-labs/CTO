// Поле «Цена прихода» с переключателем «за кан. | за л» у масла в таре (задача 003, Н7, ADR-{B6.1}):
// вторая цена считается сама (perCan/perLiter), рядом — наценка к цене продажи. Одно поле на все
// формы, где закупку вводят без прихода: новый товар, «Сканер товаров», новый товар в ревизии,
// «Указать цену закупки».
import { entryText, entryUnitPrice, markupText, otherPriceText, typed, type PriceEntry, type PricePer } from '../lib/purchasePrice'
import { Field } from './ui'

export function PurchasePriceInput({
  id,
  label = 'Цена прихода',
  entry,
  shown,
  containerMl,
  unitWord,
  sale,
  hint,
  required = false,
  autoFocus = false,
  onChange,
  onShown,
  onEnter,
}: {
  id: string
  label?: string
  entry: PriceEntry
  /** Какая цена в поле: за канистру или за литр (у товара без тары не важно). */
  shown: PricePer
  /** Тара масла в мл: есть — переключатель «за кан. | за л»; нет — цена за единицу (`unitWord`). */
  containerMl: number | null
  /** Единица цены без тары: «за шт», «за кг», «за канистру». */
  unitWord: string
  /** Цена продажи за ту же единицу (канистру, штуку, кг) — для подсказки наценки. */
  sale?: number | null
  hint?: string
  required?: boolean
  autoFocus?: boolean
  onChange: (e: PriceEntry) => void
  onShown: (m: PricePer) => void
  onEnter?: () => void
}) {
  const unitPrice = entryUnitPrice(entry, containerMl)
  const other = otherPriceText(entry, shown, containerMl)
  const markup = markupText(unitPrice, sale ?? null)
  const notes = [other, markup, hint].filter(Boolean)
  return (
    <Field
      label={`${label}${containerMl ? '' : ` ${unitWord}`}, с`}
      required={required}
      error={unitPrice === null ? 'неверная цена' : null}
      hint={
        notes.length ? (
          <span className="flex flex-col" data-testid={`${id}-hint`}>
            {notes.map((n) => (
              <span key={n}>{n}</span>
            ))}
          </span>
        ) : undefined
      }
    >
      <div className="flex items-center gap-2">
        <input
          id={id}
          className="min-w-0 flex-1"
          inputMode="decimal"
          autoFocus={autoFocus}
          value={entryText(entry, shown, containerMl)}
          onChange={(e) => onChange(typed(e.target.value, shown))}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && onEnter) {
              e.preventDefault()
              onEnter()
            }
          }}
        />
        {containerMl ? (
          <div className="inline-flex shrink-0 overflow-hidden rounded border border-slate-300 text-xs" role="radiogroup" aria-label="Цена прихода за">
            {(
              [
                ['can', 'за кан.'],
                ['l', 'за л'],
              ] as [PricePer, string][]
            ).map(([m, text]) => (
              <button
                key={m}
                type="button"
                role="radio"
                aria-checked={shown === m}
                data-testid={`${id}-per-${m}`}
                className={`px-2 py-1 ${shown === m ? 'bg-sky-600 text-white' : 'bg-white text-slate-700 hover:bg-slate-50'}`}
                onClick={() => onShown(m)}
              >
                {text}
              </button>
            ))}
          </div>
        ) : null}
      </div>
    </Field>
  )
}
