// Каркас приложения: вход, меню по ролям (боковое и нижнее на телефоне), маршруты всех экранов.
// Какой экран за какую спецификацию отвечает — docs/DEVELOPMENT.md, раздел «Карта кода».
import { Fragment, useEffect, useState, type ReactNode } from 'react'
import { BrowserRouter, NavLink, Navigate, Route, Routes, useParams } from 'react-router-dom'
import { AuthProvider, useAuth } from './lib/auth'
import { startOfflineLoop } from './lib/offline'
import { OfflineBar } from './components/OfflineBar'
import { Loading, Toaster } from './components/ui'
import { Icon, type IconName } from './components/icons'
import { get } from './lib/api'
import SettingsHub from './pages/SettingsHub'
import Cashier from './pages/Cashier'
import Categories from './pages/Categories'
import ClientCard from './pages/ClientCard'
import Debts from './pages/Debts'
import Clients from './pages/Clients'
import Employees from './pages/Employees'
import Expenses from './pages/Expenses'
import Gifts from './pages/Gifts'
import LabelSettingsPage from './pages/LabelSettings'
import Login from './pages/Login'
import Notifications from './pages/Notifications'
import Profit, { OwnerDashboard } from './pages/Owner'
import Payroll from './pages/Payroll'
import Products from './pages/Products'
import ReceiptNew from './pages/ReceiptNew'
import ReceiptView from './pages/ReceiptView'
import Receipts from './pages/Receipts'
import SaleView from './pages/SaleView'
import SalesDay from './pages/SalesDay'
import Services from './pages/Services'
import Shift from './pages/Shift'
import DebtDocsSettings from './pages/DebtDocsSettings'
import OilTransfer from './pages/OilTransfer'
import Revision from './pages/Revision'
import Stock from './pages/Stock'
import TelegramSettings from './pages/TelegramSettings'
import SupplierCard from './pages/SupplierCard'
import Suppliers from './pages/Suppliers'
import Users from './pages/Users'

interface NavItem {
  to: string
  label: string
  icon: IconName
  ownerOnly?: boolean
  adminOnly?: boolean
}

// Меню: рабочие экраны сверху, справочники и настройки ниже. Категории живут внутри «Товаров»,
// поставщики — внутри «Прихода», этикетки, документы, Телеграм и пользователи — в «Настройках».
const NAV: { title?: string; items: NavItem[] }[] = [
  {
    items: [
      { to: '/summary', label: 'Сводка', icon: 'summary', ownerOnly: true },
      { to: '/', label: 'Касса', icon: 'cashier' },
      { to: '/sales', label: 'Чеки', icon: 'receipt' },
      { to: '/shift', label: 'Смена', icon: 'shift' },
      { to: '/debts', label: 'Долги', icon: 'debts' },
      { to: '/notifications', label: 'Уведомления', icon: 'bell', ownerOnly: true },
    ],
  },
  {
    title: 'Деньги',
    items: [
      { to: '/expenses', label: 'Расходы', icon: 'expenses' },
      { to: '/payroll', label: 'Расчёт', icon: 'payroll' },
      { to: '/profit', label: 'Прибыль', icon: 'profit', ownerOnly: true },
    ],
  },
  {
    title: 'Склад',
    items: [
      { to: '/receipts', label: 'Приход', icon: 'incoming' },
      { to: '/stock', label: 'Остатки', icon: 'stock' },
      { to: '/products', label: 'Товары', icon: 'products' },
      { to: '/revision', label: 'Ревизия', icon: 'revision' },
      { to: '/oil-transfer', label: 'Перелив', icon: 'transfer', ownerOnly: true },
    ],
  },
  {
    title: 'Справочники',
    items: [
      { to: '/clients', label: 'Клиенты', icon: 'clients' },
      { to: '/employees', label: 'Сотрудники', icon: 'employees' },
      { to: '/services', label: 'Услуги', icon: 'services' },
      { to: '/gifts', label: 'Подарки', icon: 'gifts', ownerOnly: true },
      { to: '/settings', label: 'Настройки', icon: 'settings' },
    ],
  },
]

/** Нижнее меню телефона — по роли: владелец смотрит, кассир продаёт. */
const MOBILE_TABS: Record<'owner' | 'admin', NavItem[]> = {
  owner: [
    { to: '/summary', label: 'Сводка', icon: 'summary' },
    { to: '/debts', label: 'Долги', icon: 'debts' },
    { to: '/sales', label: 'Чеки', icon: 'receipt' },
    { to: '/notifications', label: 'События', icon: 'bell' },
  ],
  admin: [
    { to: '/', label: 'Касса', icon: 'cashier' },
    { to: '/sales', label: 'Чеки', icon: 'receipt' },
    { to: '/shift', label: 'Смена', icon: 'shift' },
    { to: '/stock', label: 'Остатки', icon: 'stock' },
  ],
}

const THEME_KEY = 'avtodom.theme'

/** Тема: светлая, тёмная или как в системе; выбор помнит устройство. */
function useTheme(): ['light' | 'dark' | 'system', (t: 'light' | 'dark' | 'system') => void] {
  const [theme, setTheme] = useState<'light' | 'dark' | 'system'>(() => {
    try {
      const v = localStorage.getItem(THEME_KEY)
      return v === 'light' || v === 'dark' ? v : 'system'
    } catch {
      return 'system'
    }
  })
  useEffect(() => {
    const root = document.documentElement
    if (theme === 'system') root.removeAttribute('data-theme')
    else root.setAttribute('data-theme', theme)
    try {
      if (theme === 'system') localStorage.removeItem(THEME_KEY)
      else localStorage.setItem(THEME_KEY, theme)
    } catch {
      // Без хранилища тема живёт до перезагрузки.
    }
  }, [theme])
  return [theme, setTheme]
}

/** Сколько непрочитанных уведомлений — для значка в меню (владелец). */
function useUnseen(enabled: boolean): number {
  const [n, setN] = useState(0)
  useEffect(() => {
    if (!enabled) return
    const load = () => void get<{ unseen: number }>('/notifications?limit=1').then((r) => setN(r.unseen)).catch(() => undefined)
    load()
    const t = setInterval(() => document.visibilityState === 'visible' && load(), 60_000)
    window.addEventListener('focus', load)
    return () => {
      clearInterval(t)
      window.removeEventListener('focus', load)
    }
  }, [enabled])
  return n
}

/** Пересоздаёт страницу при смене :id, чтобы окна и идентификаторы операций не переносились между документами. */
function ByParam({ children }: { children: ReactNode }) {
  const { id } = useParams()
  return <Fragment key={id}>{children}</Fragment>
}

function Shell() {
  const { user, logout } = useAuth()
  const [open, setOpen] = useState(false)
  const [theme, setTheme] = useTheme()
  const unseen = useUnseen(user?.role === 'owner')
  // Снимок каталога и очередь чеков живут, пока открыто приложение (SPEC-09).
  useEffect(() => (user ? startOfflineLoop() : undefined), [user])
  if (!user) return <Navigate to="/login" replace />
  const owner = user.role === 'owner'
  const roleLabel = owner ? 'Владелец' : 'Администратор'

  const items = (list: NavItem[]) => list.filter((it) => (owner || !it.ownerOnly) && (!owner || !it.adminOnly))
  const badge = (to: string) =>
    to === '/notifications' && unseen > 0 ? (
      <span className="ml-auto rounded-full bg-rose-600 px-1.5 text-[11px] font-semibold leading-5 text-white">{unseen > 99 ? '99+' : unseen}</span>
    ) : null

  const nav = (
    <nav className="flex flex-col gap-4">
      {NAV.map((group, i) => (
        <div key={i} className="flex flex-col gap-0.5">
          {group.title && <div className="px-3 pb-1 text-[11px] font-semibold uppercase tracking-wider text-slate-500">{group.title}</div>}
          {items(group.items).map((it) => (
            <NavLink
              key={it.to}
              to={it.to}
              end={it.to === '/'}
              onClick={() => setOpen(false)}
              className={({ isActive }) =>
                `flex items-center gap-3 rounded-md px-3 py-2 text-sm ${isActive ? 'bg-sky-600 text-white' : 'text-slate-200 hover:bg-slate-800'}`
              }
            >
              <Icon name={it.icon} className="h-[18px] w-[18px] shrink-0 opacity-90" />
              {it.label}
              {badge(it.to)}
            </NavLink>
          ))}
        </div>
      ))}
    </nav>
  )

  const themeButton = (
    <button
      type="button"
      className="inline-flex items-center gap-1.5 text-xs text-slate-300 hover:text-white"
      onClick={() => setTheme(theme === 'dark' ? 'light' : theme === 'light' ? 'system' : 'dark')}
      title="Тема оформления"
    >
      <Icon name={theme === 'dark' ? 'moon' : 'sun'} className="h-4 w-4" />
      {theme === 'dark' ? 'Тёмная' : theme === 'light' ? 'Светлая' : 'Как в системе'}
    </button>
  )

  return (
    <div className="min-h-screen md:flex">
      <aside className="keep-dark no-print hidden bg-slate-900 p-4 md:block md:w-56 md:shrink-0">
        <div className="mb-6 hidden px-3 text-lg font-bold text-white md:block">Avtodom</div>
        {nav}
        <div className="mt-6 border-t border-slate-700 px-3 pt-4 text-sm text-slate-300">
          <div className="font-medium text-white">{user.full_name}</div>
          <div className="text-xs">
            {user.login}
            {user.full_name !== roleLabel && ` · ${roleLabel}`}
          </div>
          <div className="mt-3 flex flex-wrap items-center gap-4">
            <button type="button" className="text-xs text-sky-300 hover:underline" onClick={() => void logout()}>
              Выйти
            </button>
            {themeButton}
          </div>
        </div>
      </aside>
      <div className="keep-dark no-print flex items-center justify-between bg-slate-900 px-4 py-3 text-white md:hidden">
        <span className="font-bold">Avtodom</span>
        <span className="text-xs text-slate-300">{user.full_name}</span>
      </div>
      <main className="min-w-0 flex-1 pb-24">
        <OfflineBar />
        <div className="p-4 md:p-6">
        <Routes>
          <Route path="/" element={<Cashier />} />
          <Route path="/sales" element={<SalesDay />} />
          <Route path="/sales/:id" element={<ByParam><SaleView /></ByParam>} />
          <Route path="/receipts" element={<Receipts />} />
          <Route path="/receipts/new" element={<ReceiptNew />} />
          <Route path="/receipts/:id" element={<ByParam><ReceiptView /></ByParam>} />
          <Route path="/stock" element={<Stock />} />
          <Route path="/products" element={<Products />} />
          <Route path="/shift" element={<Shift />} />
          <Route path="/expenses" element={<Expenses />} />
          <Route path="/payroll" element={<Payroll />} />
          {owner && <Route path="/summary" element={<OwnerDashboard />} />}
          {owner && <Route path="/profit" element={<Profit />} />}
          <Route path="/debts" element={<Debts />} />
          {owner && <Route path="/notifications" element={<Notifications />} />}
          <Route path="/clients" element={<Clients />} />
          <Route path="/clients/:id" element={<ByParam><ClientCard /></ByParam>} />
          <Route path="/categories" element={<Categories />} />
          <Route path="/employees" element={<Employees />} />
          <Route path="/services" element={<Services />} />
          {owner && <Route path="/gifts" element={<Gifts />} />}
          <Route path="/revision" element={<Revision />} />
          {owner && <Route path="/oil-transfer" element={<OilTransfer />} />}
          {owner && <Route path="/settings/documents" element={<DebtDocsSettings />} />}
          {owner && <Route path="/settings/telegram" element={<TelegramSettings />} />}
          <Route path="/suppliers" element={<Suppliers />} />
          <Route path="/suppliers/:id" element={<ByParam><SupplierCard /></ByParam>} />
          <Route path="/settings" element={<SettingsHub />} />
          <Route path="/settings/labels" element={<LabelSettingsPage />} />
          {owner && <Route path="/users" element={<Users />} />}
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
        </div>
      </main>
      <nav className="no-print fixed inset-x-0 bottom-0 z-30 grid grid-cols-5 border-t border-slate-200 bg-white pb-[env(safe-area-inset-bottom)] text-[11px] md:hidden">
        {MOBILE_TABS[owner ? 'owner' : 'admin'].map((t) => (
          <NavLink
            key={t.to}
            to={t.to}
            end={t.to === '/'}
            onClick={() => setOpen(false)}
            className={({ isActive }) => `relative flex min-h-[56px] flex-col items-center justify-center gap-1 ${isActive && !open ? 'text-sky-700' : 'text-slate-600'}`}
          >
            <Icon name={t.icon} className="h-6 w-6" />
            {t.label}
            {t.to === '/notifications' && unseen > 0 && (
              <span className="absolute right-[22%] top-1.5 rounded-full bg-rose-600 px-1.5 text-[10px] font-semibold leading-4 text-white">{unseen > 99 ? '99+' : unseen}</span>
            )}
          </NavLink>
        ))}
        <button
          type="button"
          aria-expanded={open}
          className={`flex min-h-[56px] flex-col items-center justify-center gap-1 ${open ? 'text-sky-700' : 'text-slate-600'}`}
          onClick={() => setOpen((o) => !o)}
        >
          <Icon name="more" className="h-6 w-6" />
          Ещё
        </button>
      </nav>
      {open && (
        <div className="no-print fixed inset-0 z-40 md:hidden" role="dialog" aria-label="Все разделы">
          <button type="button" aria-label="Закрыть" className="absolute inset-0 bg-black/50" onClick={() => setOpen(false)} />
          <div className="absolute inset-x-0 bottom-0 max-h-[85vh] overflow-y-auto rounded-t-2xl bg-white px-4 pb-[calc(env(safe-area-inset-bottom)+16px)] pt-3 shadow-2xl">
            <div className="mx-auto mb-3 h-1.5 w-10 rounded-full bg-slate-300" />
            <div className="mb-3 flex items-center justify-between">
              <div className="text-sm">
                <div className="font-semibold">{user.full_name}</div>
                <div className="text-xs text-slate-500">{roleLabel}</div>
              </div>
              <button type="button" aria-label="Закрыть" className="rounded-md p-2 text-slate-500 hover:bg-slate-100" onClick={() => setOpen(false)}>
                <Icon name="close" />
              </button>
            </div>
            {NAV.map((group, gi) => (
              <div key={gi} className="mb-3">
                {group.title && <div className="mb-1.5 text-[11px] font-semibold uppercase tracking-wider text-slate-500">{group.title}</div>}
                <div className="grid grid-cols-4 gap-2">
                  {items(group.items).map((it) => (
                    <NavLink
                      key={it.to}
                      to={it.to}
                      end={it.to === '/'}
                      onClick={() => setOpen(false)}
                      className={({ isActive }) =>
                        `relative flex flex-col items-center gap-1 rounded-xl px-1 py-2.5 text-center text-[11px] leading-tight ${isActive ? 'bg-sky-50 text-sky-700' : 'text-slate-700 hover:bg-slate-50'}`
                      }
                    >
                      <span className="flex h-10 w-10 items-center justify-center rounded-xl bg-slate-100">
                        <Icon name={it.icon} className="h-5 w-5" />
                      </span>
                      {it.label}
                      {badge(it.to) && <span className="absolute right-2 top-1.5 h-2.5 w-2.5 rounded-full bg-rose-600" />}
                    </NavLink>
                  ))}
                </div>
              </div>
            ))}
            <div className="flex items-center justify-between border-t border-slate-200 pt-3 text-sm">
              <button type="button" className="text-sky-700" onClick={() => void logout()}>
                Выйти
              </button>
              <button
                type="button"
                className="inline-flex items-center gap-1.5 text-slate-600"
                onClick={() => setTheme(theme === 'dark' ? 'light' : theme === 'light' ? 'system' : 'dark')}
              >
                <Icon name={theme === 'dark' ? 'moon' : 'sun'} className="h-4 w-4" />
                {theme === 'dark' ? 'Тёмная тема' : theme === 'light' ? 'Светлая тема' : 'Тема как в системе'}
              </button>
            </div>
          </div>
        </div>
      )}
      <Toaster />
    </div>
  )
}

function Root() {
  const { user, loading } = useAuth()
  if (loading) return <Loading />
  return (
    <Routes>
      <Route path="/login" element={user ? <Navigate to="/" replace /> : <Login />} />
      <Route path="/*" element={<Shell />} />
    </Routes>
  )
}

export default function App() {
  return (
    <AuthProvider>
      <BrowserRouter>
        <Root />
      </BrowserRouter>
    </AuthProvider>
  )
}
