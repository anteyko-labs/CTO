// «Подобрать кросс-номера» для всей категории фильтров (задача 002 П10, ADR-{B3.2}): товары по
// очереди, один подбор на товар; «Остановить» — в любой момент. «Записывать сразу» — номера уже в
// карточках; «предлагать» — список с галочками, «Сохранить отмеченные» записывает их.
import { useRef, useState } from 'react'
import { get, put } from '../lib/api'
import { addCodes, pickCross, pickProgress, splitCodes, type CrossPickResult } from '../lib/crossPick'
import { errorText } from '../lib/hooks'
import type { Category, Product } from '../lib/types'
import { Button, Checkbox, ErrorBox, Modal, Notice, toast } from './ui'

interface Row {
  product: Product
  result: CrossPickResult | null
  note: string
  chosen: string[]
}

export function CrossPickCategory({ category, onClose }: { category: Category; onClose: () => void }) {
  const [skipFilled, setSkipFilled] = useState(true)
  const [rows, setRows] = useState<Row[]>([])
  const [total, setTotal] = useState(0)
  const [running, setRunning] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [last, setLast] = useState<CrossPickResult | null>(null)
  const stop = useRef(false)

  const run = async () => {
    setError(null)
    setRows([])
    stop.current = false
    setRunning(true)
    try {
      const products = await get<Product[]>(`/products?category_id=${category.id}&limit=500`)
      setTotal(products.length)
      for (const p of products) {
        if (stop.current) break
        if (skipFilled) {
          const had = await get<{ codes: string[] }>(`/products/${p.id}/cross`)
          if (had.codes.length) {
            setRows((rs) => [...rs, { product: p, result: null, note: 'номера уже есть — пропущен', chosen: [] }])
            continue
          }
        }
        try {
          const r = await pickCross(p.id)
          setLast(r)
          const note = r.error
            ? r.error
            : r.mode === 'auto'
              ? r.added.length
                ? `записано: ${r.added.join(', ')}`
                : 'новых номеров нет'
              : r.found.length
                ? 'предложено — отметьте'
                : 'новых номеров нет'
          setRows((rs) => [...rs, { product: p, result: r, note, chosen: r.mode === 'suggest' ? r.found : [] }])
        } catch (e) {
          // Лимит дня или нет ключа — дальше не идём.
          setError(errorText(e))
          break
        }
      }
    } catch (e) {
      setError(errorText(e))
    } finally {
      setRunning(false)
    }
  }

  const offered = rows.filter((r) => r.result?.mode === 'suggest' && r.result.found.length > 0)
  const saveChosen = async () => {
    setError(null)
    try {
      let saved = 0
      for (const r of offered) {
        if (!r.chosen.length) continue
        const had = await get<{ codes: string[] }>(`/products/${r.product.id}/cross`)
        await put(`/products/${r.product.id}/cross`, { codes: splitCodes(addCodes(had.codes.join(', '), r.chosen)), auto: r.chosen })
        saved += 1
      }
      toast(saved ? `Кросс-номера сохранены у товаров: ${saved}` : 'Ничего не отмечено')
      setRows((rs) => rs.map((r) => (offered.includes(r) ? { ...r, note: r.chosen.length ? `сохранено: ${r.chosen.join(', ')}` : 'не сохранено', result: null } : r)))
    } catch (e) {
      setError(errorText(e))
    }
  }

  return (
    <Modal title={`Кросс-номера: ${category.name}`} onClose={() => !running && onClose()} wide>
      <div className="flex flex-col gap-3" data-testid="cross-pick-category">
        <p className="text-sm text-slate-600">
          Сервис разбора подбирает номера аналогов для каждого товара категории по очереди. Режим и лимит в день — «Настройки → Кросс-номера».
        </p>
        <Checkbox label="Пропускать товары, у которых номера уже есть" checked={skipFilled} onChange={setSkipFilled} />
        <div className="flex gap-2">
          {running ? (
            <Button variant="secondary" onClick={() => (stop.current = true)}>
              Остановить
            </Button>
          ) : (
            <Button onClick={() => void run()}>{rows.length ? 'Подобрать ещё раз' : 'Подобрать'}</Button>
          )}
        </div>
        {(running || rows.length > 0) && <div className="text-sm text-slate-600">{pickProgress(rows.length, total, last)}</div>}
        <ErrorBox error={error} />
        {rows.length > 0 && (
          <ul className="max-h-[50dvh] divide-y divide-slate-100 overflow-y-auto rounded-md border border-slate-200 text-sm">
            {rows.map((r) => (
              <li key={r.product.id} className="px-3 py-2">
                <div className="flex flex-wrap justify-between gap-2">
                  <span className="font-medium">{r.product.name}</span>
                  <span className="text-slate-500">{r.note}</span>
                </div>
                {r.result?.mode === 'suggest' && r.result.found.length > 0 && (
                  <div className="mt-1 flex flex-wrap gap-3">
                    {r.result.found.map((c) => (
                      <Checkbox
                        key={c}
                        label={c}
                        checked={r.chosen.includes(c)}
                        onChange={(v) =>
                          setRows((rs) => rs.map((x) => (x === r ? { ...x, chosen: v ? [...x.chosen, c] : x.chosen.filter((y) => y !== c) } : x)))
                        }
                      />
                    ))}
                  </div>
                )}
              </li>
            ))}
          </ul>
        )}
        {offered.length > 0 && !running && (
          <div className="flex justify-end">
            <Button onClick={() => void saveChosen()}>Сохранить отмеченные</Button>
          </div>
        )}
        {rows.some((r) => r.result?.mode === 'auto' && r.result.added.length > 0) && (
          <Notice tone="sky">Записанные номера помечены «подобрано автоматически»; лишний уберите в карточке товара.</Notice>
        )}
      </div>
    </Modal>
  )
}
