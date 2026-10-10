// Настройки одним экраном: этикетки, документы, Телеграм и баллы, пользователи, категории и поставщики.
import { Link } from 'react-router-dom'
import { Icon, type IconName } from '../components/icons'
import { PageHeader } from '../components/ui'
import { useUser } from '../lib/auth'

const ITEMS: { to: string; title: string; text: string; icon: IconName; ownerOnly?: boolean }[] = [
  { to: '/settings/labels', title: 'Этикетки', text: 'Размер этикетки, что печатать: название, цена, штрихкод', icon: 'products' },
  { to: '/settings/documents', title: 'Документы о долге', text: 'Тексты расписки, накладной и акта сверки', icon: 'receipt', ownerOnly: true },
  { to: '/settings/telegram', title: 'Телеграм и баллы', text: 'Бот владельца, процент баллов клиентам', icon: 'bell', ownerOnly: true },
  { to: '/users', title: 'Пользователи', text: 'Кто входит в систему: владелец и администраторы', icon: 'employees', ownerOnly: true },
  { to: '/categories', title: 'Категории товаров', text: 'Масла, фильтры, аккумуляторы и их характеристики', icon: 'stock' },
  { to: '/suppliers', title: 'Поставщики', text: 'У кого покупаем, долги поставщикам', icon: 'incoming' },
]

export default function SettingsHub() {
  const owner = useUser().role === 'owner'
  return (
    <div>
      <PageHeader title="Настройки" />
      <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
        {ITEMS.filter((it) => owner || !it.ownerOnly).map((it) => (
          <Link
            key={it.to}
            to={it.to}
            className="group flex items-start gap-3 rounded-lg border border-slate-200 bg-white p-4 shadow-sm transition hover:border-sky-400 hover:shadow"
          >
            <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-sky-50 text-sky-700">
              <Icon name={it.icon} />
            </span>
            <span className="min-w-0 flex-1">
              <span className="block font-medium">{it.title}</span>
              <span className="block text-sm text-slate-500">{it.text}</span>
            </span>
            <Icon name="chevron" className="mt-2 h-4 w-4 text-slate-400 group-hover:text-sky-600" />
          </Link>
        ))}
      </div>
    </div>
  )
}
