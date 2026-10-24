import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from 'react'
import { ApiError, get, noServerAnswer, post, setUnauthorizedHandler } from './api'
import { rememberBranch, selectedBranch } from './branch'
import type { User } from './types'

interface AuthState {
  user: User | null
  loading: boolean
  login: (login: string, password: string) => Promise<void>
  logout: () => Promise<void>
}

const AuthContext = createContext<AuthState | null>(null)

const USER_KEY = 'avtodom.user'

/** Последний вошедший — чтобы касса открылась без сети после перезагрузки. */
function remember(u: User | null): void {
  try {
    if (u) localStorage.setItem(USER_KEY, JSON.stringify(u))
    else localStorage.removeItem(USER_KEY)
  } catch {
    // Без хранилища офлайн-запуск недоступен, остальное работает.
  }
}

function remembered(): User | null {
  try {
    const raw = localStorage.getItem(USER_KEY)
    return raw ? (JSON.parse(raw) as User) : null
  } catch {
    return null
  }
}

/**
 * Текущий пользователь с филиалом (SPEC-33). Выбранного на устройстве филиала больше нет (или
 * он чужой: вошёл администратор) — выбор забывается, ответ берётся по своему филиалу.
 */
async function loadMe(): Promise<User> {
  let u: User
  try {
    u = await get<User>('/auth/me')
  } catch (e) {
    if (selectedBranch() && e instanceof ApiError && e.status === 422) {
      rememberBranch(null)
      return get<User>('/auth/me')
    }
    throw e
  }
  if (u.role !== 'owner' && selectedBranch()) rememberBranch(null)
  return u
}

export function AuthProvider({ children }: { children: ReactNode }) {
  const [user, setUser] = useState<User | null>(null)
  const [loading, setLoading] = useState(true)

  useEffect(() => {
    setUnauthorizedHandler(() => {
      remember(null)
      setUser(null)
    })
    loadMe()
      .then((u) => {
        remember(u)
        setUser(u)
      })
      .catch((e: unknown) => {
        // Сервер не ответил (касса включилась без сети или сервер перезапускается) — работаем
        // под тем, кто входил последним: касса продаёт офлайн, чеки уйдут позже (SPEC-09).
        if (noServerAnswer(e)) setUser(remembered())
        else if (!(e instanceof ApiError && e.status === 401)) console.error(e)
      })
      .finally(() => setLoading(false))
  }, [])

  const login = useCallback(async (login: string, password: string) => {
    await post<User>('/auth/login', { login, password })
    // Ответ входа — в своём филиале; выбор филиала на устройстве проверяет `/auth/me`.
    const u = await loadMe()
    remember(u)
    setUser(u)
  }, [])

  const logout = useCallback(async () => {
    await post('/auth/logout').catch(() => undefined)
    remember(null)
    setUser(null)
  }, [])

  return <AuthContext.Provider value={{ user, loading, login, logout }}>{children}</AuthContext.Provider>
}

export function useAuth(): AuthState {
  const ctx = useContext(AuthContext)
  if (!ctx) throw new Error('useAuth вне AuthProvider')
  return ctx
}

/** Текущий пользователь на страницах за входом. */
export function useUser(): User {
  const { user } = useAuth()
  if (!user) throw new Error('нет пользователя')
  return user
}

