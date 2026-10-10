// Ссылка «← Назад» в шапке справочника: категории и поставщики открываются из настроек.
import { Link } from 'react-router-dom'
import { Icon } from './icons'

export function StockBackLink({ to = '/settings', label = 'Настройки' }: { to?: string; label?: string }) {
  return (
    <Link to={to} className="inline-flex items-center gap-1 text-sky-700 hover:text-sky-800">
      <Icon name="back" className="h-4 w-4" />
      {label}
    </Link>
  )
}
