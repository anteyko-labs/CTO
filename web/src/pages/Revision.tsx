// Ревизия склада: пересчёт сканером по категориям, проведение владельцем (SPEC-15).
import { useState } from 'react'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, Modal, PageHeader, Table, toast } from '../components/ui'
import { del, get, newOpId, post, put } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatDateTime, formatLiters, formatSom, parseLiters } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import type { Category } from '../lib/types'

interface Head {
  id: string
  number: number
  category_id: string | null
  category_name: string | null
  status: 'draft' | 'posted' | 'cancelled'
  comment: string
  user_name: string
  created_at: string
  posted_at: string | null
  counted: number
}

interface Line {
  product_id: string
  name: string
  article: string
  barcodes: string[]
  unit: 'piece' | 'ml'
  container_ml: number | null
  expected_qty: number
  counted_qty: number | null
  value_delta_tyiyn?: number
}

interface RevisionOut {
  head: Head
  lines: Line[]
}

const STATUS: Record<Head['status'], [string, 'amber' | 'green' | 'slate']> = {
  draft: ['идёт пересчёт', 'amber'],
  posted: ['проведена', 'green'],
  cancelled: ['отменена', 'slate'],
}

const qtyText = (l: Pick<Line, 'unit'>, q: number) => (l.unit === 'ml' ? formatLiters(q) : `${q} шт`)

/** Поле пересчёта: штуки целым числом, масло литрами с запятой. */
function parseCount(l: Line, text: string): number | null {
  const t = text.trim()
  if (!t) return null
  if (l.unit === 'ml') return parseLiters(t)
  return /^\d+$/.test(t) ? Number(t) : null
}

function Draft({ rev, onChanged }: { rev: RevisionOut; onChanged: () => void }) {
  const owner = useUser().role === 'owner'
  const [scan, setScan] = useState('')
  const [filter, setFilter] = useState('')
  const [edits, setEdits] = useState<Record<string, string>>({})
  const [posting, setPosting] = useState<{ comment: string; opId: string } | null>(null)
  const act = useAction()
  const draft = rev.head.status === 'draft'

  const save = (l: Line, qty: number | null) =>
    void act.run(async () => {
      if (qty === null) await del(`/revisions/${rev.head.id}/lines/${l.product_id}`)
      else await put(`/revisions/${rev.head.id}/lines`, { product_id: l.product_id, counted_qty: qty })
      setEdits((e) => {
        const next = { ...e }
        delete next[l.product_id]
        return next
      })
      onChanged()
    })

  // Скан: штучный товар +1, у масла — фокус на поле литров.
  const onScan = () => {
    const code = scan.trim()
    if (!code) return
    const l = rev.lines.find((x) => x.barcodes.includes(code))
    setScan('')
    if (!l) {
      act.setError(`Код ${code} не найден в этой ревизии${rev.head.category_name ? ` (категория «${rev.head.category_name}»)` : ''}`)
      return
    }
    act.setError(null)
    if (l.unit === 'piece') save(l, (l.counted_qty ?? 0) + 1)
    else document.querySelector<HTMLInputElement>(`[data-count="${l.product_id}"]`)?.focus()
  }

  const q = filter.trim().toLowerCase()
  const lines = q ? rev.lines.filter((l) => l.name.toLowerCase().includes(q) || l.article.toLowerCase().includes(q)) : rev.lines
  const counted = rev.lines.filter((l) => l.counted_qty !== null)
  const diffs = counted.filter((l) => l.counted_qty !== l.expected_qty)
  const valueTotal = rev.lines.reduce((acc, l) => acc + (l.value_delta_tyiyn ?? 0), 0)

  return (
    <div className="flex flex-col gap-4">
      {draft && (
        <Card className="flex flex-col gap-3">
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label="Сканер" hint="Скан штучного товара прибавляет 1; у масла введите литры в строке">
              <input
                autoFocus
                data-picker
                value={scan}
                onChange={(e) => setScan(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && onScan()}
                placeholder="Сканируйте штрихкод"
              />
            </Field>
            <Field label="Найти в списке">
              <input value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="Название или артикул" />
            </Field>
          </div>
          <div className="text-sm text-slate-600">
            Пересчитано {counted.length} из {rev.lines.length}, расхождений {diffs.length}. Непересчитанные товары не меняются; не нашли товар —
            впишите 0.
          </div>
        </Card>
      )}
      <ErrorBox error={act.error} />
      <Card className="p-0">
        {lines.length === 0 ? (
          <Empty>Товаров нет</Empty>
        ) : (
          <Table head={['Товар', 'Должно быть', 'Пересчитано', 'Разница', ...(owner && !draft ? ['В деньгах'] : [])]}>
            {lines.map((l) => {
              const diff = l.counted_qty === null ? null : l.counted_qty - l.expected_qty
              const edit = edits[l.product_id]
              return (
                <tr key={l.product_id}>
                  <td className="px-2 py-2">
                    <div className="font-medium">{l.name}</div>
                    {l.article && <div className="text-xs text-slate-500">{l.article}</div>}
                  </td>
                  <td className="whitespace-nowrap px-2 py-2">{qtyText(l, l.expected_qty)}</td>
                  <td className="whitespace-nowrap px-2 py-2">
                    {draft ? (
                      <input
                        data-count={l.product_id}
                        className="w-24"
                        inputMode={l.unit === 'ml' ? 'decimal' : 'numeric'}
                        placeholder={l.unit === 'ml' ? 'л' : 'шт'}
                        value={edit ?? (l.counted_qty === null ? '' : l.unit === 'ml' ? formatLiters(l.counted_qty).replace(/\s*л$/, '') : String(l.counted_qty))}
                        onChange={(e) => setEdits({ ...edits, [l.product_id]: e.target.value })}
                        onKeyDown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()}
                        onBlur={() => {
                          if (edit === undefined) return
                          const v = parseCount(l, edit)
                          if (edit.trim() && v === null) {
                            act.setError(`Неверное количество: ${l.name}`)
                            return
                          }
                          save(l, v)
                        }}
                      />
                    ) : l.counted_qty === null ? (
                      '—'
                    ) : (
                      qtyText(l, l.counted_qty)
                    )}
                  </td>
                  <td className={`whitespace-nowrap px-2 py-2 font-medium ${diff === null || diff === 0 ? 'text-slate-400' : diff < 0 ? 'text-rose-700' : 'text-emerald-700'}`}>
                    {diff === null ? '—' : diff === 0 ? 'сходится' : `${diff > 0 ? '+' : '−'}${qtyText(l, Math.abs(diff))}`}
                  </td>
                  {owner && !draft && (
                    <td className="whitespace-nowrap px-2 py-2">{l.value_delta_tyiyn ? formatSom(l.value_delta_tyiyn) : '—'}</td>
                  )}
                </tr>
              )
            })}
          </Table>
        )}
      </Card>
      {owner && !draft && rev.head.status === 'posted' && (
        <div className="text-right font-semibold">Итог ревизии в деньгах: {formatSom(valueTotal)}</div>
      )}
      {draft && (
        <div className="flex flex-wrap justify-end gap-2">
          <Button
            variant="secondary"
            disabled={act.busy}
            onClick={() =>
              void act.run(async () => {
                await post(`/revisions/${rev.head.id}/cancel`)
                onChanged()
              })
            }
          >
            Отменить ревизию
          </Button>
          {owner ? (
            <Button disabled={act.busy || counted.length === 0} onClick={() => setPosting({ comment: '', opId: newOpId() })}>
              Провести
            </Button>
          ) : (
            <span className="self-center text-sm text-slate-500">Провести ревизию может владелец</span>
          )}
        </div>
      )}
      {posting && (
        <Modal title={`Провести ревизию № ${rev.head.number}`} onClose={() => setPosting(null)}>
          <div className="flex flex-col gap-3">
            <div className="text-sm text-slate-600">
              Остатки {counted.length} товаров станут равны пересчитанным, расхождений — {diffs.length}. Недостача уменьшит прибыль по средней
              себестоимости. Проведённую ревизию не изменить.
            </div>
            <Field label="Кто пересчитывал, причина" required>
              <input autoFocus value={posting.comment} onChange={(e) => setPosting({ ...posting, comment: e.target.value })} />
            </Field>
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setPosting(null)}>
                Отмена
              </Button>
              <Button
                disabled={act.busy || !posting.comment.trim()}
                onClick={() =>
                  void act.run(async () => {
                    await post(`/revisions/${rev.head.id}/post`, { op_id: posting.opId, comment: posting.comment })
                    setPosting(null)
                    onChanged()
                    toast('Ревизия проведена')
                  })
                }
              >
                Провести
              </Button>
            </div>
          </div>
        </Modal>
      )}
    </div>
  )
}

export default function Revision() {
  const list = useLoad(() => get<Head[]>('/revisions'), [])
  const categories = useLoad(() => get<Category[]>('/categories'), [])
  const [openId, setOpenId] = useState<string | null>(null)
  const rev = useLoad(() => (openId ? get<RevisionOut>(`/revisions/${openId}`) : Promise.resolve(null)), [openId])
  const [category, setCategory] = useState('')
  const act = useAction()

  const start = () =>
    void act.run(async () => {
      const r = await post<RevisionOut>('/revisions', { category_id: category || null })
      list.reload()
      setOpenId(r.head.id)
    })

  if (openId && rev.data) {
    const h = rev.data.head
    return (
      <div className="flex flex-col gap-4">
        <PageHeader
          title={`Ревизия № ${h.number}${h.category_name ? ` · ${h.category_name}` : ' · весь склад'}`}
          actions={
            <Button
              variant="secondary"
              onClick={() => {
                setOpenId(null)
                list.reload()
              }}
            >
              К списку
            </Button>
          }
        />
        <Draft
          rev={rev.data}
          onChanged={() => {
            rev.reload()
            list.reload()
          }}
        />
      </div>
    )
  }

  return (
    <div className="flex flex-col gap-4">
      <PageHeader title="Ревизия" />
      <Card className="flex flex-wrap items-end gap-3">
        <Field label="Что пересчитываем">
          <select value={category} onChange={(e) => setCategory(e.target.value)}>
            <option value="">Весь склад</option>
            {(categories.data ?? []).map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </select>
        </Field>
        <Button disabled={act.busy} onClick={start}>
          Начать ревизию
        </Button>
        <div className="text-xs text-slate-500">Раз в месяц: пересчитайте сканером, владелец проведёт и остатки выровняются.</div>
      </Card>
      <ErrorBox error={list.error ?? act.error ?? rev.error} />
      <Card className="p-0">
        {list.loading && !list.data ? (
          <Loading />
        ) : (list.data ?? []).length === 0 ? (
          <Empty>Ревизий ещё не было</Empty>
        ) : (
          <Table head={['№', 'Что', 'Начал', 'Пересчитано', 'Статус', '']}>
            {(list.data ?? []).map((h) => (
              <tr key={h.id} className="cursor-pointer hover:bg-slate-50" onClick={() => setOpenId(h.id)}>
                <td className="px-2 py-2 font-medium">{h.number}</td>
                <td className="px-2 py-2">{h.category_name ?? 'Весь склад'}</td>
                <td className="whitespace-nowrap px-2 py-2">
                  {h.user_name}, {formatDateTime(h.created_at)}
                </td>
                <td className="px-2 py-2">{h.counted}</td>
                <td className="px-2 py-2">
                  <Badge tone={STATUS[h.status][1]}>{STATUS[h.status][0]}</Badge>
                </td>
                <td className="px-2 py-2 text-right text-sky-700">Открыть</td>
              </tr>
            ))}
          </Table>
        )}
      </Card>
    </div>
  )
}
