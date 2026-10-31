// Настройки одним экраном: касса (задача 003 Н2–Н4), этикетки, склад (цена масла за канистру или литр), документы, Телеграм и баллы, уведомления в бот, книжка и печать, кросс-номера, Optima Business, комиссия банка, статьи расходов,
// филиалы, пользователи, категории и поставщики. Владельцу — выключатель старой базы Умаг (SPEC-36 §15).
import { Link } from 'react-router-dom'
import { Icon, type IconName } from '../components/icons'
import { Badge, Button, Card, ErrorBox, PageHeader, toast } from '../components/ui'
import { useUser } from '../lib/auth'
import { can } from '../lib/permissions'
import type { Permission } from '../lib/types'
import { useAction } from '../lib/hooks'
import { saveUmagSwitch, useUmagSwitch } from '../lib/umagSwitch'

const ITEMS: { to: string; title: string; text: string; icon: IconName; ownerOnly?: boolean; permission?: Permission }[] = [
  { to: '/settings/cashier', title: 'Касса', text: 'Кнопки режима кассы и их место, скидка и её предел, прибыль в чеке смены', icon: 'cashier', ownerOnly: true },
  { to: '/settings/labels', title: 'Этикетки', text: 'Размер этикетки, что печатать: название, цена, штрихкод', icon: 'products' },
  { to: '/settings/stock', title: 'Склад', text: 'Цена масла в приходе: за канистру или за литр; срок залежалости', icon: 'stock', ownerOnly: true },
  { to: '/settings/documents', title: 'Документы о долге', text: 'Тексты расписки, накладной и акта сверки', icon: 'receipt', ownerOnly: true },
  { to: '/settings/telegram', title: 'Телеграм и баллы', text: 'Бот владельца, процент баллов клиентам', icon: 'bell', ownerOnly: true },
  { to: '/settings/bot-feed', title: 'Уведомления в бот', text: 'Что приходит вам в Телеграм: чеки, склад, деньги, долги, зарплата, смена', icon: 'bell', ownerOnly: true },
  { to: '/settings/oil-book', title: 'Масляная книжка и печать', text: 'Что печатать после чека «В сервис», фильтры в книжке, масло канистрой и разливом', icon: 'receipt', ownerOnly: true },
  { to: '/settings/cross-pick', title: 'Кросс-номера', text: 'Подбор номеров аналогов фильтров: записывать сразу или предлагать, лимит в день', icon: 'products', ownerOnly: true },
  { to: '/settings/client-messages', title: 'Сообщения клиентам', text: 'Напоминания о замене и сообщения юрлицам о закупе: Телеграм и WhatsApp, журнал', icon: 'bell', ownerOnly: true },
  { to: '/settings/optima', title: 'Optima Business', text: 'QR на сумму чека и сверка с выпиской банка: компания, счёт, точка и касса', icon: 'bank', ownerOnly: true },
  { to: '/accounts', title: 'Комиссия банка', text: 'Ставка у каждого банковского счёта и история ставок — в «Обзоре счетов»', icon: 'profit', ownerOnly: true },
  { to: '/settings/payroll', title: 'Зарплата', text: 'Дата выплаты задним числом, выплаты без смены, оклад на сегодня, напоминание в день расчёта', icon: 'employees', ownerOnly: true },
  { to: '/settings/expense-articles', title: 'Статьи расходов', text: 'Добавить, переименовать, скрыть; личные — только владельцу', icon: 'expenses', ownerOnly: true },
  { to: '/settings/branches', title: 'Филиалы', text: 'Филиалы на общем сервере, новый филиал, закрытие; расчёты между филиалами', icon: 'stock', ownerOnly: true },
  { to: '/users', title: 'Пользователи и права', text: 'Кто входит в систему и что можно каждому администратору', icon: 'employees', ownerOnly: true },
  { to: '/categories', title: 'Категории товаров', text: 'Масла, фильтры, аккумуляторы и их характеристики', icon: 'stock' },
  { to: '/suppliers', title: 'Поставщики', text: 'У кого покупаем, долги поставщикам', icon: 'truck', permission: 'view_suppliers' },
]

export default function SettingsHub() {
  const user = useUser()
  const owner = user.role === 'owner'
  return (
    <div>
      <PageHeader title="Настройки" />
      <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
        {ITEMS.filter((it) => (owner || !it.ownerOnly) && (!it.permission || can(user, it.permission))).map((it) => (
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
      {owner && <UmagSwitchCard />}
    </div>
  )
}

/**
 * «Старая база Умаг: включена / выключена» (ADR-126). Выключенный Умаг не подсказывает и не
 * связывает; перенесённые товарам коды остаются, список и связи хранятся — включается обратно.
 */
function UmagSwitchCard() {
  const s = useUmagSwitch()
  const act = useAction()
  const toggle = (enabled: boolean) =>
    void act.run(async () => {
      await saveUmagSwitch(enabled)
      toast(enabled ? 'Старая база Умаг включена' : 'Старая база Умаг выключена — коды у товаров остались')
    })
  return (
    <Card className="mt-4" data-testid="umag-switch">
      <div className="flex flex-wrap items-center gap-3">
        <span className="font-medium">Старая база Умаг:</span>
        {s ? <Badge tone={s.enabled ? 'green' : 'slate'}>{s.enabled ? 'включена' : 'выключена'}</Badge> : <span className="text-sm text-slate-500">…</span>}
        {s && (
          <span className="text-sm text-slate-500">
            строк {s.rows.toLocaleString('ru-RU')}, связей с товарами {s.linked.toLocaleString('ru-RU')}
          </span>
        )}
        {s && (
          <Button variant="secondary" className="ml-auto" disabled={act.busy} onClick={() => toggle(!s.enabled)}>
            {s.enabled ? 'Выключить' : 'Включить'}
          </Button>
        )}
      </div>
      <p className="mt-2 text-sm text-slate-500">
        Выключенная база не подсказывает в «Товарах», приходе, ревизии и «Сканере товаров», связывать с ней нельзя. Коды, уже перенесённые
        товарам, остаются — старые наклейки сканируются, в том числе без сети.
      </p>
      <ErrorBox error={act.error} />
    </Card>
  )
}
