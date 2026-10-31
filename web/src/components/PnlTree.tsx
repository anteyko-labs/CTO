import { formatDate } from '../lib/format'
import type { PnlNode } from '../lib/pnl'
import { Badge, Money } from './ui'

/**
 * Дерево «Прибыли и убытков» (задача 003 Н12): строка с «›» раскрывается до подгрупп и
 * документов. Справа сумма; справочные строки — серым курсивом; у документа — дата, кому, касса
 * и кто провёл. На телефоне — те же строки, подробности переносятся под название.
 */
export function PnlTree({ nodes, open, onToggle }: { nodes: PnlNode[]; open: Set<string>; onToggle: (id: string) => void }) {
  return (
    <div className="divide-y divide-slate-100 text-sm" role="tree">
      {nodes.map((n) => (
        <PnlRow key={n.id} node={n} level={0} open={open} onToggle={onToggle} />
      ))}
    </div>
  )
}

function PnlRow({ node, level, open, onToggle }: { node: PnlNode; level: number; open: Set<string>; onToggle: (id: string) => void }) {
  const kids = node.children ?? []
  const canOpen = kids.length > 0 || (node.hidden ?? 0) > 0
  const isOpen = canOpen && open.has(node.id)
  const d = node.doc
  const details = d ? [d.date && formatDate(d.date), d.payee && `кому: ${d.payee}`, d.account, d.user, d.comment && d.payee !== d.comment && d.comment, d.branch].filter(Boolean) : []
  return (
    <div role="treeitem" aria-expanded={canOpen ? isOpen : undefined} data-testid={`pnl-${node.id}`}>
      <button
        type="button"
        disabled={!canOpen}
        onClick={() => onToggle(node.id)}
        className={`flex w-full items-start gap-2 py-2 pr-1 text-left disabled:cursor-default ${canOpen ? 'hover:bg-slate-50' : ''} ${node.total ? 'border-t border-slate-200 text-base font-bold' : level === 0 ? 'font-semibold' : ''} ${node.reversal ? 'text-slate-400' : node.info ? 'italic text-slate-500' : ''}`}
        style={{ paddingLeft: `${level * 1.25 + 0.25}rem` }}
      >
        <span className="w-4 shrink-0 text-slate-400" aria-hidden>
          {canOpen ? (isOpen ? '▾' : '›') : ''}
        </span>
        <span className="min-w-0 flex-1">
          <span className="break-words">{node.label}</span>
          {d?.mark && (
            <span className="ml-2 align-middle">
              <Badge tone="rose">{d.mark}</Badge>
            </span>
          )}
          {details.length > 0 && <span className="block text-xs font-normal text-slate-500">{details.join(' · ')}</span>}
        </span>
        {node.amount !== null && <Money value={node.amount} tone="negative" className={node.total ? '' : 'font-normal'} />}
      </button>
      {isOpen && (
        <div>
          {kids.map((c) => (
            <PnlRow key={c.id} node={c} level={level + 1} open={open} onToggle={onToggle} />
          ))}
          {(node.hidden ?? 0) > 0 && (
            <div className="py-1 text-xs text-slate-400" style={{ paddingLeft: `${(level + 1) * 1.25 + 1.5}rem` }}>
              Скрыто строк сторно и исправлений: {node.hidden} (пары гасятся, сумма не меняется)
            </div>
          )}
        </div>
      )}
    </div>
  )
}
