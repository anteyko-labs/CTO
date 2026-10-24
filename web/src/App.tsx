// Каркас приложения: вход, меню по ролям (боковое и нижнее на телефоне), маршруты всех экранов.
// Какой экран за какую спецификацию отвечает — docs/DEVELOPMENT.md, раздел «Карта кода».
import { Fragment, useEffect, useState, type ReactNode } from 'react'
import { BrowserRouter, NavLink, Navigate, Route, Routes, useLocation, useParams } from 'react-router-dom'
import { AuthProvider, useAuth } from './lib/auth'
import { can } from './lib/permissions'
import type { Permission } from './lib/types'
import { logoutWarning, outboxItems, startOfflineLoop } from './lib/offline'
import { useTouchMode } from './lib/cashierMode'
import { stockToProducts } from './lib/stockLink'
import { Keypad } from './components/Keypad'
import { OfflineBar } from './components/OfflineBar'
import { BranchSwitch } from './components/BranchSwitch'
import { setBranchLabel } from './lib/branch'
import { UpdateBanner } from './components/UpdateBanner'
import { startUpdates } from './lib/updates'
import { Button, Loading, Modal, Toaster } from './components/ui'
import { Icon, type IconName } from './components/icons'
import { get, serverReachable } from './lib/api'
import SettingsHub from './pages/SettingsHub'
import Cashier from './pages/Cashier'
import Accounts from './pages/Accounts'
import Categories from './pages/Categories'
import ClientCard from './pages/ClientCard'
import Bank from './pages/Bank'
import BankRecon from './pages/BankRecon'
import OptimaSettings from './pages/OptimaSettings'
import Debts from './pages/Debts'
import Income from './pages/Income'
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
import TopProducts from './pages/TopProducts'
import ReceiptCorrect from './pages/ReceiptCorrect'
import ReceiptNew from './pages/ReceiptNew'
import ReceiptView from './pages/ReceiptView'
import Receipts from './pages/Receipts'
import SaleCorrect from './pages/SaleCorrect'
import SaleView from './pages/SaleView'
import SalesDay from './pages/SalesDay'
import Services from './pages/Services'
import Shift from './pages/Shift'
import ShiftReport from './pages/ShiftReport'
import DebtDocsSettings from './pages/DebtDocsSettings'
import OilTransfer from './pages/OilTransfer'
import Revision from './pages/Revision'
import StockInCorrect from './pages/StockInCorrect'
import StockInView from './pages/StockInView'
import SupplierReturnsPage from './pages/SupplierReturns'
import TelegramSettings from './pages/TelegramSettings'
import ClientMessages from './pages/ClientMessages'
import ExpenseArticles from './pages/ExpenseArticles'
import SupplierCard from './pages/SupplierCard'
import Suppliers from './pages/Suppliers'
import Users from './pages/Users'
import Branches from './pages/Branches'

interface NavItem {
  to: string
  label: string
  icon: IconName
  ownerOnly?: boolean
  adminOnly?: boolean
  /** Пункт для владельца и администратора с этим правом (ADR-082, ADR-097); список — с любым из прав. */
  permission?: Permission | Permission[]
}

// Меню: рабочие экраны сверху, справочники и настройки ниже. Категории живут внутри «Товаров»,
// поставщики — в «Складе» (и в «Настройках»), этикетки, документы, Телеграм и пользователи — в «Настройках».
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
      { to: '/expenses', label: 'Расходы', icon: 'expenses', permission: 'expenses' },
      { to: '/payroll', label: 'Расчёт', icon: 'payroll' },
      { to: '/profit', label: 'Прибыль', icon: 'profit', ownerOnly: true },
      { to: '/income', label: 'Поступления', icon: 'income' },
      { to: '/accounts', label: 'Счета', icon: 'wallet' },
      { to: '/money', label: 'Внести / Изъять', icon: 'plus', permission: 'cash_moves' },
      { to: '/bank', label: 'Безнал (банк)', icon: 'bank', ownerOnly: true },
    ],
  },
  {
    title: 'Склад',
    items: [
      { to: '/receipts', label: 'Приход', icon: 'incoming', permission: ['receipts', 'stock_in'] },
      { to: '/suppliers', label: 'Поставщики', icon: 'truck' },
      { to: '/supplier-returns', label: 'Возврат поставщику', icon: 'supplier-return', permission: 'supplier_returns' },
      // «Остатки» — в «Товарах» (SPEC-30 §11).
      { to: '/products', label: 'Товары', icon: 'products' },
      { to: '/top-products', label: 'Продаваемые', icon: 'star' },
      { to: '/revision', label: 'Ревизия и товары', icon: 'revision', permission: ['revision', 'products'] },
      { to: '/oil-transfer', label: 'Перелив', icon: 'transfer', permission: 'oil_transfer' },
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
    { to: '/products', label: 'Товары', icon: 'stock' },
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

/** «Остатки» вошли в «Товары» (SPEC-30 §11): старый адрес — туда же, с теми же отборами. */
function StockRedirect() {
  const { search } = useLocation()
  return <Navigate to={stockToProducts(search)} replace />
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
  // Название филиала в печатных чеках — когда филиалов несколько (SPEC-33).
  useEffect(() => setBranchLabel(user?.several_branches ? (user.branch_name ?? null) : null), [user])
  // Снимок каталога и очередь чеков живут, пока открыто приложение (SPEC-09).
  useEffect(() => (user ? startOfflineLoop({ id: user.id, full_name: user.full_name, role: user.role }) : undefined), [user])
  // Выход с неотправленными чеками или без связи — сначала предупреждение (SPEC-09): очередь
  // остаётся за этим пользователем, без связи войти снова не получится.
  const [logoutAsk, setLogoutAsk] = useState<string | null>(null)
  const askLogout = () => {
    if (!user) return
    void outboxItems().then((items) => {
      const warning = logoutWarning(items, user.id, navigator.onLine && serverReachable())
      if (warning) setLogoutAsk(warning)
      else void logout()
    })
  }
  // Режим кассы (SPEC-29): на экране «Касса» — без меню, всё крупнее, цифры экранной клавиатурой.
  const touch = useTouchMode()
  useEffect(() => {
    document.documentElement.classList.toggle('cashier-mode', touch)
    return () => document.documentElement.classList.remove('cashier-mode')
  }, [touch])
  if (!user) return <Navigate to="/login" replace />
  const owner = user.role === 'owner'
  const roleLabel = owner ? 'Владелец' : 'Администратор'

  // Пункт по праву (ADR-097): без права администратор его не видит.
  const allowed = (p?: Permission | Permission[]) => !p || (Array.isArray(p) ? p : [p]).some((x) => can(user, x))
  const items = (list: NavItem[]) =>
    list.filter((it) => (owner || !it.ownerOnly) && (!owner || !it.adminOnly) && allowed(it.permission))
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
      {!touch && (
      <aside className="keep-dark no-print hidden bg-slate-900 p-4 md:block md:w-56 md:shrink-0">
        <div className="mb-6 hidden px-3 md:block">
          <div className="text-lg font-bold text-white">Avtodom</div>
          <div className="mt-1">
            <BranchSwitch user={user} />
          </div>
        </div>
        {nav}
        <div className="mt-6 border-t border-slate-700 px-3 pt-4 text-sm text-slate-300">
          <div className="font-medium text-white">{user.full_name}</div>
          <div className="text-xs">
            {user.login}
            {user.full_name !== roleLabel && ` · ${roleLabel}`}
          </div>
          <div className="mt-3 flex flex-wrap items-center gap-4">
            <button type="button" className="text-xs text-sky-300 hover:underline" onClick={askLogout}>
              Выйти
            </button>
            {themeButton}
          </div>
        </div>
      </aside>
      )}
      {!touch && (
        <div className="keep-dark no-print flex items-center justify-between bg-slate-900 px-4 py-3 text-white md:hidden">
          <span className="font-bold">Avtodom</span>
          {user.several_branches ? <BranchSwitch user={user} /> : <span className="text-xs text-slate-300">{user.full_name}</span>}
        </div>
      )}
      {/* Режим кассы на широком экране — ровно во весь экран без прокрутки страницы (ui-rules §9). */}
      <main className={`min-w-0 flex-1 ${touch ? 'pb-3 lg:flex lg:h-dvh lg:flex-col lg:overflow-hidden lg:pb-0' : 'pb-24'}`}>
        <UpdateBanner />
        <OfflineBar />
        {logoutAsk && (
          <Modal title="Выйти из программы?" onClose={() => setLogoutAsk(null)}>
            <div className="flex flex-col gap-4" data-testid="logout-warning">
              <div className="text-sm text-slate-700">{logoutAsk}</div>
              <div className="flex justify-end gap-2">
                <Button variant="secondary" onClick={() => setLogoutAsk(null)}>
                  Остаться
                </Button>
                <Button
                  variant="danger"
                  onClick={() => {
                    setLogoutAsk(null)
                    void logout()
                  }}
                >
                  Выйти, чеки останутся на устройстве
                </Button>
              </div>
            </div>
          </Modal>
        )}
        <div className={touch ? 'px-3 pb-3 md:px-4 lg:min-h-0 lg:flex-1' : 'p-4 md:p-6'}>
        <Routes>
          <Route path="/" element={<Cashier />} />
          <Route path="/sales" element={<SalesDay />} />
          <Route path="/sales/:id" element={<ByParam><SaleView /></ByParam>} />
          {owner && <Route path="/sales/:id/correct" element={<ByParam><SaleCorrect /></ByParam>} />}
          <Route path="/receipts" element={<Receipts />} />
          <Route path="/receipts/new" element={<ReceiptNew />} />
          <Route path="/receipts/:id" element={<ByParam><ReceiptView /></ByParam>} />
          {owner && <Route path="/receipts/:id/correct" element={<ByParam><ReceiptCorrect /></ByParam>} />}
          <Route path="/stock" element={<StockRedirect />} />
          <Route path="/products" element={<Products />} />
          <Route path="/top-products" element={<TopProducts />} />
          <Route path="/intake" element={<Navigate to="/revision?tab=scan" replace />} />
          <Route path="/shift" element={<Shift />} />
          <Route path="/shifts/:id" element={<ByParam><ShiftReport /></ByParam>} />
          {can(user, 'expenses') && <Route path="/expenses" element={<Expenses />} />}
          <Route path="/payroll" element={<Payroll />} />
          {owner && <Route path="/summary" element={<OwnerDashboard />} />}
          {owner && <Route path="/profit" element={<Profit />} />}
          {owner && <Route path="/bank" element={<Bank />} />}
          {owner && <Route path="/bank/reconcile" element={<BankRecon />} />}
          <Route path="/income" element={<Income />} />
          <Route path="/accounts" element={<Accounts />} />
          {can(user, 'cash_moves') && <Route path="/money" element={<Accounts key="money" openMoney />} />}
          {/* Оприходование — вариант прихода (ADR-086): список — «Приход» с отбором. */}
          <Route path="/stock-ins" element={<Navigate to="/receipts?kind=stock_in" replace />} />
          <Route path="/stock-ins/:id" element={<ByParam><StockInView /></ByParam>} />
          {owner && <Route path="/stock-ins/:id/correct" element={<ByParam><StockInCorrect /></ByParam>} />}
          {can(user, 'supplier_returns') && <Route path="/supplier-returns" element={<SupplierReturnsPage />} />}
          <Route path="/debts" element={<Debts />} />
          {owner && <Route path="/notifications" element={<Notifications />} />}
          <Route path="/clients" element={<Clients />} />
          <Route path="/clients/:id" element={<ByParam><ClientCard /></ByParam>} />
          <Route path="/categories" element={<Categories />} />
          <Route path="/employees" element={<Employees />} />
          <Route path="/services" element={<Services />} />
          {owner && <Route path="/gifts" element={<Gifts />} />}
          <Route path="/revision" element={<Revision />} />
          {can(user, 'oil_transfer') && <Route path="/oil-transfer" element={<OilTransfer />} />}
          {owner && <Route path="/settings/documents" element={<DebtDocsSettings />} />}
          {owner && <Route path="/settings/telegram" element={<TelegramSettings />} />}
          {owner && <Route path="/settings/optima" element={<OptimaSettings />} />}
          {owner && <Route path="/settings/client-messages" element={<ClientMessages />} />}
          {owner && <Route path="/settings/expense-articles" element={<ExpenseArticles />} />}
          <Route path="/suppliers" element={<Suppliers />} />
          <Route path="/suppliers/:id" element={<ByParam><SupplierCard /></ByParam>} />
          <Route path="/settings" element={<SettingsHub />} />
          <Route path="/settings/labels" element={<LabelSettingsPage />} />
          {owner && <Route path="/users" element={<Users />} />}
          {owner && <Route path="/settings/branches" element={<Branches />} />}
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
        </div>
      </main>
      {!touch && (
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
      )}
      {open && !touch && (
        <div className="no-print fixed inset-0 z-40 md:hidden" role="dialog" aria-label="Все разделы">
          <button type="button" aria-label="Закрыть" className="absolute inset-0 bg-black/50" onClick={() => setOpen(false)} />
          <div className="absolute inset-x-0 bottom-0 max-h-[85vh] overflow-y-auto rounded-t-2xl bg-white px-4 pb-[calc(env(safe-area-inset-bottom)+16px)] pt-3 shadow-2xl">
            <div className="mx-auto mb-3 h-1.5 w-10 rounded-full bg-slate-300" />
            <div className="mb-3 flex items-center justify-between">
              <div className="text-sm">
                <div className="font-semibold">{user.full_name}</div>
                {user.full_name !== roleLabel && <div className="text-xs text-slate-500">{roleLabel}</div>}
                <div className="mt-1">
                  <BranchSwitch user={user} tone="light" />
                </div>
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
              <button type="button" className="text-sky-700" onClick={askLogout}>
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
      {touch && <Keypad />}
      <Toaster />
    </div>
  )
}

function Root() {
  const { user, loading } = useAuth()
  // Service Worker — с первого экрана, и до входа: касса, открытая на экране входа без сети,
  // иначе не получит оболочку для офлайна и новую версию (ADR-059).
  useEffect(() => startUpdates(), [])
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
