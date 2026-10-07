import { Fragment, useState, type ReactNode } from 'react'
import { BrowserRouter, NavLink, Navigate, Route, Routes, useParams } from 'react-router-dom'
import { AuthProvider, useAuth } from './lib/auth'
import { Loading, Toaster } from './components/ui'
import Cashier from './pages/Cashier'
import Categories from './pages/Categories'
import ClientCard from './pages/ClientCard'
import Debts from './pages/Debts'
import Clients from './pages/Clients'
import Employees from './pages/Employees'
import Gifts from './pages/Gifts'
import LabelSettingsPage from './pages/LabelSettings'
import Login from './pages/Login'
import Notifications from './pages/Notifications'
import Products from './pages/Products'
import ReceiptNew from './pages/ReceiptNew'
import ReceiptView from './pages/ReceiptView'
import Receipts from './pages/Receipts'
import SaleView from './pages/SaleView'
import SalesDay from './pages/SalesDay'
import Services from './pages/Services'
import Stock from './pages/Stock'
import SupplierCard from './pages/SupplierCard'
import Suppliers from './pages/Suppliers'
import Users from './pages/Users'

interface NavItem {
  to: string
  label: string
  ownerOnly?: boolean
}

const NAV: { title?: string; items: NavItem[] }[] = [
  {
    items: [
      { to: '/', label: 'Касса' },
      { to: '/sales', label: 'Чеки' },
      { to: '/debts', label: 'Долги' },
      { to: '/notifications', label: 'Уведомления', ownerOnly: true },
    ],
  },
  {
    title: 'Склад',
    items: [
      { to: '/receipts', label: 'Приход' },
      { to: '/stock', label: 'Остатки' },
      { to: '/products', label: 'Товары' },
    ],
  },
  {
    title: 'Справочники',
    items: [
      { to: '/clients', label: 'Клиенты' },
      { to: '/categories', label: 'Категории' },
      { to: '/employees', label: 'Сотрудники' },
      { to: '/services', label: 'Услуги' },
      { to: '/gifts', label: 'Подарки', ownerOnly: true },
      { to: '/suppliers', label: 'Поставщики' },
    ],
  },
  {
    title: 'Настройки',
    items: [
      { to: '/settings/labels', label: 'Этикетки' },
      { to: '/users', label: 'Пользователи', ownerOnly: true },
    ],
  },
]

/** Пересоздаёт страницу при смене :id, чтобы окна и идентификаторы операций не переносились между документами. */
function ByParam({ children }: { children: ReactNode }) {
  const { id } = useParams()
  return <Fragment key={id}>{children}</Fragment>
}

function Shell() {
  const { user, logout } = useAuth()
  const [open, setOpen] = useState(false)
  if (!user) return <Navigate to="/login" replace />
  const owner = user.role === 'owner'
  const roleLabel = owner ? 'Владелец' : 'Администратор'

  const nav = (
    <nav className="flex flex-col gap-4">
      {NAV.map((group, i) => (
        <div key={i} className="flex flex-col gap-1">
          {group.title && <div className="px-3 text-xs font-semibold uppercase text-slate-400">{group.title}</div>}
          {group.items
            .filter((it) => owner || !it.ownerOnly)
            .map((it) => (
              <NavLink
                key={it.to}
                to={it.to}
                end={it.to === '/'}
                onClick={() => setOpen(false)}
                className={({ isActive }) =>
                  `rounded-md px-3 py-2 text-sm ${isActive ? 'bg-sky-600 text-white' : 'text-slate-200 hover:bg-slate-800'}`
                }
              >
                {it.label}
              </NavLink>
            ))}
        </div>
      ))}
    </nav>
  )

  return (
    <div className="min-h-screen md:flex">
      <aside className={`no-print bg-slate-900 p-4 md:block md:w-56 md:shrink-0 ${open ? 'block' : 'hidden'}`}>
        <div className="mb-6 hidden px-3 text-lg font-bold text-white md:block">Avtodom</div>
        {nav}
        <div className="mt-6 border-t border-slate-700 px-3 pt-4 text-sm text-slate-300">
          <div className="font-medium text-white">{user.full_name}</div>
          <div className="text-xs">
            {user.login}
            {user.full_name !== roleLabel && ` · ${roleLabel}`}
          </div>
          <button type="button" className="mt-2 text-xs text-sky-300 hover:underline" onClick={() => void logout()}>
            Выйти
          </button>
        </div>
      </aside>
      <div className="no-print flex items-center justify-between bg-slate-900 px-4 py-3 text-white md:hidden">
        <span className="font-bold">Avtodom</span>
        <span className="text-xs text-slate-300">{user.full_name}</span>
      </div>
      <main className="min-w-0 flex-1 p-4 pb-24 md:p-6">
        <Routes>
          <Route path="/" element={<Cashier />} />
          <Route path="/sales" element={<SalesDay />} />
          <Route path="/sales/:id" element={<ByParam><SaleView /></ByParam>} />
          <Route path="/receipts" element={<Receipts />} />
          <Route path="/receipts/new" element={<ReceiptNew />} />
          <Route path="/receipts/:id" element={<ByParam><ReceiptView /></ByParam>} />
          <Route path="/stock" element={<Stock />} />
          <Route path="/products" element={<Products />} />
          <Route path="/debts" element={<Debts />} />
          {owner && <Route path="/notifications" element={<Notifications />} />}
          <Route path="/clients" element={<Clients />} />
          <Route path="/clients/:id" element={<ByParam><ClientCard /></ByParam>} />
          <Route path="/categories" element={<Categories />} />
          <Route path="/employees" element={<Employees />} />
          <Route path="/services" element={<Services />} />
          {owner && <Route path="/gifts" element={<Gifts />} />}
          <Route path="/suppliers" element={<Suppliers />} />
          <Route path="/suppliers/:id" element={<ByParam><SupplierCard /></ByParam>} />
          <Route path="/settings/labels" element={<LabelSettingsPage />} />
          {owner && <Route path="/users" element={<Users />} />}
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </main>
      <nav className="no-print fixed inset-x-0 bottom-0 z-30 grid grid-cols-5 border-t border-slate-200 bg-white text-xs md:hidden">
        {MOBILE_TABS.map((t) => (
          <NavLink
            key={t.to}
            to={t.to}
            end={t.to === '/'}
            onClick={() => setOpen(false)}
            className={({ isActive }) => `flex flex-col items-center gap-0.5 py-2 ${isActive ? 'text-sky-700' : 'text-slate-600'}`}
          >
            <span className="text-lg leading-none" aria-hidden>
              {t.icon}
            </span>
            {t.label}
          </NavLink>
        ))}
        <button
          type="button"
          className={`flex flex-col items-center gap-0.5 py-2 ${open ? 'text-sky-700' : 'text-slate-600'}`}
          onClick={() => {
            setOpen((o) => !o)
            window.scrollTo({ top: 0 })
          }}
        >
          <span className="text-lg leading-none" aria-hidden>
            ☰
          </span>
          Ещё
        </button>
      </nav>
      <Toaster />
    </div>
  )
}

/** Нижнее меню телефона: самые частые экраны, остальное — в «Ещё». */
const MOBILE_TABS = [
  { to: '/', label: 'Касса', icon: '▣' },
  { to: '/sales', label: 'Чеки', icon: '≡' },
  { to: '/stock', label: 'Остатки', icon: '▤' },
  { to: '/receipts', label: 'Приход', icon: '⇩' },
]

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
