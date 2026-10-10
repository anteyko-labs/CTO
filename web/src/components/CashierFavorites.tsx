// Ходовые товары и услуги плитками над чеком: одно касание — строка в чеке. Набор задаёт владелец.
import { useState } from 'react'
import { get, put } from '../lib/api'
import { useUser } from '../lib/auth'
import { useAction, useLoad } from '../lib/hooks'
import type { FavoriteTile, Product, Service } from '../lib/types'
import { Icon } from './icons'
import { ProductPicker, stockText } from './ProductPicker'
import { Button, ErrorBox, Modal, Money, toast } from './ui'

const MAX_TILES = 24

interface EditItem {
  kind: 'product' | 'service'
  id: string
  name: string
  price: number
}

function toEdit(t: FavoriteTile): EditItem | null {
  if (t.kind === 'product' && t.product) return { kind: 'product', id: t.product.id, name: t.product.name, price: t.product.sale_price_tyiyn }
  if (t.kind === 'service' && t.service) return { kind: 'service', id: t.service.id, name: t.service.name, price: t.service.price_tyiyn }
  return null
}

export function CashierFavorites({ onProduct, onService }: { onProduct: (p: Product) => void; onService: (s: Service) => void }) {
  const owner = useUser().role === 'owner'
  const tiles = useLoad(() => get<FavoriteTile[]>('/settings/favorites'), [])
  const [editing, setEditing] = useState(false)
  const list = (tiles.data ?? []).filter((t) => (t.kind === 'product' ? t.product : t.service))

  // Пока грузится или без сети — плиток нет, касса работает поиском.
  if (!tiles.data && !editing) return null
  if (list.length === 0 && !owner) return null

  return (
    <div className="flex flex-col gap-2">
      {list.length > 0 && owner && (
        <div className="flex justify-end">
          <button type="button" className="text-xs text-sky-700 hover:underline" onClick={() => setEditing(true)}>
            Настроить
          </button>
        </div>
      )}
      <div className="grid grid-cols-2 gap-2 sm:grid-cols-4 xl:grid-cols-5 2xl:grid-cols-6">
        {list.map((t) => {
          const p = t.product
          const s = t.service
          const out = p ? p.stock_qty <= 0 : false
          return (
            <button
              key={`${t.kind}-${p?.id ?? s?.id}`}
              type="button"
              className="flex min-h-19 flex-col justify-between rounded-lg border border-slate-200 bg-white p-2.5 text-left shadow-sm transition hover:border-sky-400 hover:bg-sky-50 active:bg-sky-100"
              onClick={() => (p ? onProduct(p) : s && onService(s))}
            >
              <span className="line-clamp-2 text-sm font-medium leading-snug">{p?.name ?? s?.name}</span>
              <span className="mt-1 flex items-baseline justify-between gap-2">
                <Money value={p ? p.sale_price_tyiyn : (s?.price_tyiyn ?? 0)} className="font-semibold text-sky-700" />
                <span className={`truncate text-xs ${out ? 'text-rose-700' : 'text-slate-500'}`}>{p ? stockText(p) : 'услуга'}</span>
              </span>
            </button>
          )
        })}
        {list.length === 0 && owner && (
          <button
            type="button"
            className="col-span-2 flex min-h-16 items-center justify-center gap-2 rounded-lg border-2 border-dashed border-slate-300 text-sm text-slate-500 hover:border-sky-400 hover:text-sky-700"
            onClick={() => setEditing(true)}
          >
            <Icon name="plus" className="h-4 w-4" />
            Добавить ходовые товары
          </button>
        )}
      </div>
      {editing && (
        <FavoritesEditor
          initial={list.map(toEdit).filter((x): x is EditItem => x !== null)}
          onClose={() => setEditing(false)}
          onSaved={() => {
            setEditing(false)
            tiles.reload()
          }}
        />
      )}
    </div>
  )
}

function FavoritesEditor({ initial, onClose, onSaved }: { initial: EditItem[]; onClose: () => void; onSaved: () => void }) {
  const [items, setItems] = useState<EditItem[]>(initial)
  const [serviceQuery, setServiceQuery] = useState('')
  const services = useLoad(() => get<Service[]>('/services'), [])
  const { busy, error, setError, run } = useAction()

  const add = (it: EditItem) => {
    setError(null)
    if (items.some((x) => x.id === it.id)) return
    if (items.length >= MAX_TILES) {
      setError(`Плиток не больше ${MAX_TILES}`)
      return
    }
    setItems((xs) => [...xs, it])
  }
  const move = (i: number, dir: -1 | 1) =>
    setItems((xs) => {
      const j = i + dir
      if (j < 0 || j >= xs.length) return xs
      const next = xs.filter((_, k) => k !== i)
      next.splice(j, 0, xs[i])
      return next
    })

  const q = serviceQuery.trim().toLowerCase()
  const shownServices = (services.data ?? []).filter((s) => s.active && !items.some((x) => x.id === s.id) && (!q || s.name.toLowerCase().includes(q)))

  const save = () =>
    void run(async () => {
      await put('/settings/favorites', items.map(({ kind, id }) => ({ kind, id })))
      toast('Плитки сохранены')
      onSaved()
    })

  return (
    <Modal title="Ходовые товары" onClose={onClose} wide>
      <div className="flex flex-col gap-4">
        <div className="flex flex-col gap-1">
          <span className="text-sm font-medium text-slate-700">Добавить товар</span>
          <ProductPicker
            autoFocus={false}
            placeholder="Название, бренд или штрихкод"
            onPick={(p) => add({ kind: 'product', id: p.id, name: p.name, price: p.sale_price_tyiyn })}
          />
        </div>
        <div className="flex flex-col gap-1">
          <span className="text-sm font-medium text-slate-700">Добавить услугу</span>
          <input placeholder="Найти услугу" value={serviceQuery} onChange={(e) => setServiceQuery(e.target.value)} />
          {shownServices.length > 0 && (
            <div className="flex max-h-32 flex-wrap gap-1.5 overflow-y-auto">
              {shownServices.map((s) => (
                <button
                  key={s.id}
                  type="button"
                  className="rounded-md border border-slate-300 bg-white px-2 py-1 text-xs hover:border-sky-500"
                  onClick={() => add({ kind: 'service', id: s.id, name: s.name, price: s.price_tyiyn })}
                >
                  + {s.name}
                </button>
              ))}
            </div>
          )}
        </div>
        <div className="flex flex-col gap-1">
          <span className="text-sm font-medium text-slate-700">
            Плитки в кассе ({items.length} из {MAX_TILES})
          </span>
          {items.length === 0 ? (
            <div className="rounded-md border border-dashed border-slate-300 px-3 py-4 text-center text-sm text-slate-500">
              Пока пусто — добавьте товары и услуги, которые продаются чаще всего.
            </div>
          ) : (
            <ul className="divide-y divide-slate-100 rounded-md border border-slate-200">
              {items.map((it, i) => (
                <li key={it.id} className="flex items-center gap-2 px-2 py-1.5 text-sm">
                  <span className="w-6 text-right text-xs text-slate-400">{i + 1}</span>
                  <span className="min-w-0 flex-1 truncate">
                    {it.name}
                    {it.kind === 'service' && <span className="ml-1 text-xs text-slate-500">· услуга</span>}
                  </span>
                  <Money value={it.price} className="text-xs text-slate-600" />
                  <button
                    type="button"
                    className="h-8 w-8 rounded text-slate-500 hover:bg-slate-100 disabled:opacity-30"
                    aria-label="Выше"
                    disabled={i === 0}
                    onClick={() => move(i, -1)}
                  >
                    ↑
                  </button>
                  <button
                    type="button"
                    className="h-8 w-8 rounded text-slate-500 hover:bg-slate-100 disabled:opacity-30"
                    aria-label="Ниже"
                    disabled={i === items.length - 1}
                    onClick={() => move(i, 1)}
                  >
                    ↓
                  </button>
                  <button
                    type="button"
                    className="h-8 w-8 rounded text-slate-400 hover:bg-rose-50 hover:text-rose-600"
                    aria-label="Убрать плитку"
                    onClick={() => setItems((xs) => xs.filter((x) => x.id !== it.id))}
                  >
                    ✕
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
        <ErrorBox error={error ?? services.error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={busy} onClick={save}>
            Сохранить
          </Button>
        </div>
      </div>
    </Modal>
  )
}
