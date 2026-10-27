// Выключатель старой базы Умаг (SPEC-36 §15, ADR-126). Выключенный Умаг сервер не ищет и не
// связывает; здесь — чтобы экраны прятали свои кнопки и блоки Умага. Пока состояние не известно,
// блоки не показываются: подсказки по поиску всё равно приходят с сервера, а он уже знает.
import { useEffect, useState } from 'react'
import { get, put } from './api'
import type { UmagSwitch } from './types'

let current: UmagSwitch | null = null
let pending: Promise<UmagSwitch | null> | null = null
const subs = new Set<(s: UmagSwitch | null) => void>()

function publish(s: UmagSwitch | null) {
  current = s
  for (const fn of subs) fn(s)
}

/** Перечитать состояние; одновременные вызовы — один запрос. Без связи — прежнее. */
export function loadUmagSwitch(): Promise<UmagSwitch | null> {
  pending ??= get<UmagSwitch>('/settings/old-catalog')
    .then((s) => {
      publish(s)
      return s
    })
    .catch(() => current)
    .finally(() => {
      pending = null
    })
  return pending
}

/** Включить или выключить (только владелец). */
export async function saveUmagSwitch(enabled: boolean): Promise<UmagSwitch> {
  const s = await put<UmagSwitch>('/settings/old-catalog', { enabled })
  publish(s)
  return s
}

/**
 * Состояние выключателя: null — ещё не известно. Перечитывается при каждом показе экрана;
 * `active = false` — экрану Умаг не нужен (касса), запроса нет.
 */
export function useUmagSwitch(active = true): UmagSwitch | null {
  const [s, setS] = useState(current)
  useEffect(() => {
    if (!active) return
    subs.add(setS)
    void loadUmagSwitch()
    return () => {
      subs.delete(setS)
    }
  }, [active])
  return s
}

/** Умаг включён и это известно — показывать кнопки и блоки Умага. */
export const useUmagOn = (active = true): boolean => useUmagSwitch(active)?.enabled === true
