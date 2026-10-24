// Переключатель филиала в шапке (SPEC-33 §1): владелец выбирает филиал, выбор помнит устройство;
// у администратора — только название его филиала. Один филиал — ничего не показывается.
import { switchBranch } from '../lib/branch'
import type { User } from '../lib/types'

export function BranchSwitch({ user, tone = 'dark' }: { user: User; tone?: 'dark' | 'light' }) {
  if (!user.several_branches) return null
  const text = tone === 'dark' ? 'text-slate-300' : 'text-slate-600'
  if (user.role !== 'owner' || !user.branches || user.branches.length < 2) {
    return (
      <div className={`text-xs ${text}`} data-testid="branch-name">
        Филиал: {user.branch_name}
      </div>
    )
  }
  return (
    <label className={`flex items-center gap-1.5 text-xs ${text}`}>
      <span>Филиал:</span>
      <select
        data-testid="branch-switch"
        aria-label="Филиал"
        className={`min-w-0 max-w-[11rem] truncate rounded border px-1.5 py-1 text-xs ${
          tone === 'dark' ? 'border-slate-600 bg-slate-800 text-white' : 'border-slate-300 bg-white text-slate-800'
        }`}
        value={user.branch_id}
        onChange={(e) => switchBranch(e.target.value)}
      >
        {user.branches.map((b) => (
          <option key={b.id} value={b.id}>
            {b.name}
            {b.archived ? ' (закрыт)' : ''}
          </option>
        ))}
      </select>
    </label>
  )
}
