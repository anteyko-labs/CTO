// Администратор в боте (SPEC-40): что он видит в Телеграме по своим правам и какие события ему
// приходят. Пункты и события с подписями — с сервера (`GET /users/{id}/bot`), здесь — типы и
// правка списка включённого. Денег владельца в боте администратора нет (I13).

export interface AdminBotItem {
  key: string
  button: string
  what: string
  /** Пункт есть в меню администратора сейчас. */
  on: boolean
  /** Чего не хватает: «Нет права: …». */
  need?: string
}

export interface AdminBotEvent {
  action: string
  label: string
  on: boolean
  /** Право события есть: без него включённое не приходит. */
  allowed: boolean
  need?: string
}

export interface AdminBotGroup {
  key: string
  title: string
  events: AdminBotEvent[]
}

export interface AdminBot {
  linked: boolean
  linked_at: string | null
  /** Телеграм-имя привязанного чата: @имя или имя в Телеграме (ADR-{h18}). */
  tg_name?: string | null
  items: AdminBotItem[]
  on: string[]
  groups: AdminBotGroup[]
}

/** Список включённого после переключения одного события: без повторов, порядок прежний. */
export function switchAdminEvent(on: string[], action: string, value: boolean): string[] {
  const rest = on.filter((a) => a !== action)
  return value ? [...rest, action] : rest
}

/** Экран после сохранения: отметки событий — по списку включённого. */
export function applyAdminFeed(bot: AdminBot, on: string[]): AdminBot {
  return {
    ...bot,
    on,
    groups: bot.groups.map((g) => ({ ...g, events: g.events.map((e) => ({ ...e, on: on.includes(e.action) })) })),
  }
}

/** «2 из 15» — сколько событий включено. */
export function enabledCount(bot: AdminBot): string {
  const all = bot.groups.reduce((n, g) => n + g.events.length, 0)
  return `${bot.on.length} из ${all}`
}
