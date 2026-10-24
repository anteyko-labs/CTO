// Отчёт по выбранному филиалу или «Все филиалы» (SPEC-33 §3) и разбивка суммы по филиалам.
import type { ReactNode } from 'react'
import type { BranchPart } from '../lib/branchReport'
import { formatSom } from '../lib/format'
import type { User } from '../lib/types'
import { Card, CardTitle } from './ui'

/** Переключатель «Филиал / Все филиалы»: только владельцу и только при нескольких филиалах. */
export function BranchScope({ user, all, onChange }: { user: User; all: boolean; onChange: (all: boolean) => void }) {
  if (user.role !== 'owner' || !user.several_branches) return null
  const btn = (on: boolean, label: ReactNode, v: boolean, testid: string) => (
    <button
      type="button"
      data-testid={testid}
      aria-pressed={on}
      onClick={() => onChange(v)}
      className={`rounded-md px-3 py-1.5 text-sm ${on ? 'bg-sky-600 text-white' : 'text-slate-700 hover:bg-slate-100'}`}
    >
      {label}
    </button>
  )
  return (
    <div className="mb-3 inline-flex rounded-lg border border-slate-200 bg-white p-0.5" role="group" aria-label="Филиал отчёта">
      {btn(!all, `Филиал «${user.branch_name ?? ''}»`, false, 'scope-branch')}
      {btn(all, 'Все филиалы', true, 'scope-all')}
    </div>
  )
}

/** Разбивка «Все филиалы»: строка на филиал и итог — те же числа, что в сумме выше. */
export function BranchBreakdown<T>({
  parts,
  columns,
  title = 'По филиалам',
}: {
  parts: BranchPart<T>[] | undefined
  columns: { label: string; value: (r: T) => number }[]
  title?: string
}) {
  if (!parts?.length) return null
  const total = (c: (r: T) => number) => parts.reduce((s, p) => s + c(p.report), 0)
  return (
    <Card className="mb-4" data-testid="branch-breakdown">
      <CardTitle>{title}</CardTitle>
      <div className="overflow-x-auto">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-slate-200 text-left text-xs uppercase text-slate-500">
              <th className="px-2 py-2 font-medium">Филиал</th>
              {columns.map((c) => (
                <th key={c.label} className="px-2 py-2 text-right font-medium">
                  {c.label}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {parts.map((p) => (
              <tr key={p.branch_id} className={`border-b border-slate-100 ${p.archived ? 'text-slate-400' : ''}`}>
                <td className="px-2 py-2">
                  {p.name}
                  {p.archived ? ' (закрыт)' : ''}
                </td>
                {columns.map((c) => (
                  <td key={c.label} className="px-2 py-2 text-right tabular-nums">
                    {formatSom(c.value(p.report))}
                  </td>
                ))}
              </tr>
            ))}
            <tr className="font-semibold">
              <td className="px-2 py-2">Все филиалы</td>
              {columns.map((c) => (
                <td key={c.label} className="px-2 py-2 text-right tabular-nums">
                  {formatSom(total(c.value))}
                </td>
              ))}
            </tr>
          </tbody>
        </table>
      </div>
    </Card>
  )
}
