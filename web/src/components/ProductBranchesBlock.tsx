// «Продаётся в филиалах» в карточке товара (задача 036, ADR-197): карточка общая, список товаров
// у филиала свой. Отмечает только владелец — он один видит все филиалы. Товар с остатком в филиале
// убрать нельзя — сервер ответит 409 с причиной (I6).
import { get, newOpId, patch } from '../lib/api'
import { useUser } from '../lib/auth'
import { useAction, useLoad } from '../lib/hooks'
import { Checkbox, ErrorBox } from './ui'

interface ProductBranch {
  branch_id: string
  name: string
  listed: boolean
}

export function ProductBranchesBlock({ productId }: { productId: string }) {
  const user = useUser()
  const show = user.role === 'owner' && Boolean(user.several_branches)
  const rows = useLoad(() => (show ? get<ProductBranch[]>(`/products/${productId}/branches`) : Promise.resolve([])), [productId, show])
  const { busy, error, run } = useAction()
  if (!show || !rows.data?.length) return null
  const toggle = (b: ProductBranch, listed: boolean) =>
    void run(async () => {
      if (busy) return
      const out = await patch<ProductBranch[]>(`/products/${productId}/branches`, { op_id: newOpId(), branch_id: b.branch_id, listed })
      rows.setData(out)
    })
  return (
    <div className="mt-4 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-sm" data-testid="product-branches">
      <div className="mb-1 font-medium text-slate-700">Продаётся в филиалах</div>
      <div className="flex flex-wrap gap-x-5 gap-y-1">
        {rows.data.map((b) => (
          <Checkbox key={b.branch_id} label={b.name} checked={b.listed} onChange={(v) => toggle(b, v)} />
        ))}
      </div>
      <p className="mt-1 text-xs text-slate-500">Без отметки товара нет в кассе, на складе и в приходе этого филиала.</p>
      <ErrorBox error={error} />
    </div>
  )
}
