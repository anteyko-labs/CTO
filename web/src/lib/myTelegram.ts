// «Мой Телеграм» администратора (SPEC-40 §10, ADR-172): состояние привязки своего чата, что он
// увидит в боте по своим правам, и подсказка «Подключите бот» на главной — один раз на устройстве.
import type { AdminBotItem } from './adminBot'

export interface MyTelegram {
  /** Ключ бота задан на сервере. */
  enabled: boolean
  /** Владелец разрешил администраторам привязывать Телеграм самим. */
  self_link: boolean
  linked: boolean
  linked_at: string | null
  /** @имя или имя в Телеграме привязанного чата. */
  tg_name: string | null
  items: AdminBotItem[]
  /** Подписи событий, которые ему приходят (включил владелец). */
  events: string[]
}

/** Что показать вместо кнопки: можно взять код, привязку делает владелец, бот выключен. */
export type LinkAction = 'code' | 'owner' | 'off'

export function linkAction(t: Pick<MyTelegram, 'enabled' | 'self_link'>): LinkAction {
  if (!t.enabled) return 'off'
  return t.self_link ? 'code' : 'owner'
}

/** «Привязан с 01.11.2026, 10:00 к @erbol» — дата уже в виде для людей. */
export function linkedText(t: Pick<MyTelegram, 'linked' | 'tg_name'>, when: string | null): string {
  if (!t.linked) return 'Телеграм не привязан'
  const at = when ? ` с ${when}` : ''
  const to = t.tg_name ? ` к ${t.tg_name}` : ''
  return `Привязан${at}${to}`
}

/** Ключ отметки «подсказку уже показали» — у каждого пользователя на устройстве свой. */
export function hintKey(userId: string): string {
  return `avtodom.botHint.${userId}`
}

/** Подсказку «Подключите бот» показывать: бот включён, чат не привязан, привязать можно самому,
 *  на этом устройстве её ещё не показывали. */
export function showBotHint(t: MyTelegram | null, seen: boolean): boolean {
  return !!t && !seen && t.enabled && t.self_link && !t.linked
}
