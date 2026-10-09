import { useCallback, useEffect, useRef, useState } from 'react'
import { ApiError } from './api'

export function errorText(e: unknown): string {
  if (e instanceof ApiError) return e.message
  if (e instanceof Error) return e.message
  return 'Неизвестная ошибка'
}

/** Загрузка данных; повторяется при смене `deps` и по `reload`. */
export function useLoad<T>(load: () => Promise<T>, deps: unknown[]) {
  const [data, setData] = useState<T | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [tick, setTick] = useState(0)
  const loadRef = useRef(load)
  loadRef.current = load

  useEffect(() => {
    let alive = true
    setLoading(true)
    loadRef
      .current()
      .then((d) => {
        if (alive) {
          setData(d)
          setError(null)
        }
      })
      .catch((e: unknown) => {
        if (alive) setError(errorText(e))
      })
      .finally(() => {
        if (alive) setLoading(false)
      })
    return () => {
      alive = false
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, tick])

  const reload = useCallback(() => setTick((t) => t + 1), [])
  return { data, error, loading, reload, setData }
}

/** Выполнение действия с состоянием «занято» и текстом ошибки. */
export function useAction() {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const run = useCallback(async <T>(fn: () => Promise<T>): Promise<T | undefined> => {
    setBusy(true)
    setError(null)
    try {
      return await fn()
    } catch (e) {
      setError(errorText(e))
      return undefined
    } finally {
      setBusy(false)
    }
  }, [])

  return { busy, error, setError, run }
}

/** Отложенное значение для поиска по мере ввода. */
export function useDebounced<T>(value: T, ms = 250): T {
  const [v, setV] = useState(value)
  useEffect(() => {
    const t = setTimeout(() => setV(value), ms)
    return () => clearTimeout(t)
  }, [value, ms])
  return v
}

/** Живое обновление: перезапрос раз в `ms`, пока вкладка на экране (ADR-028). */
export function usePolling(reload: () => void, ms = 15_000) {
  useEffect(() => {
    const t = setInterval(() => {
      if (document.visibilityState === 'visible') reload()
    }, ms)
    return () => clearInterval(t)
  }, [reload, ms])
}
