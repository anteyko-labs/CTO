// «Настройки → Уведомления в бот» (задача 002 П11, ADR-138): что из событий приходит владельцу в
// Телеграм. Список событий и подписи — с сервера (`GET /settings/bot-feed`), здесь — типы и
// правка списка выключенных. Экран «Уведомления» в программе настройка не меняет.

/** Чеки и возвраты: каждый; только в долг и возвраты; не присылать (по умолчанию). */
export type SalesMode = 'all' | 'debt_returns' | 'none'

export const SALES_MODES: { value: SalesMode; label: string }[] = [
  { value: 'all', label: 'Каждый чек' },
  { value: 'debt_returns', label: 'Только в долг и возвраты' },
  { value: 'none', label: 'Не присылать' },
]

export interface BotFeedEvent {
  action: string
  label: string
  on: boolean
}

export interface BotFeedGroup {
  key: string
  title: string
  events: BotFeedEvent[]
}

export interface BotFeed {
  off: string[]
  sales: SalesMode
  /** Себестоимость и прибыль в сообщениях о чеках и возвратах (задача 003 Н4); у старого ответа
   *  сервера поля нет — значит, да. */
  profit?: boolean
  groups: BotFeedGroup[]
}

/** Список выключенных после переключения одного события: без повторов, порядок прежний. */
export function switchEvent(off: string[], action: string, on: boolean): string[] {
  const rest = off.filter((a) => a !== action)
  return on ? rest : [...rest, action]
}

/** Настройка с новым списком выключенных, режимом чеков и прибылью в чеках: выключатели групп — по
 *  списку. */
export function applyFeed(feed: BotFeed, off: string[], sales: SalesMode, profit: boolean = feed.profit ?? true): BotFeed {
  return {
    off,
    sales,
    profit,
    groups: feed.groups.map((g) => ({ ...g, events: g.events.map((e) => ({ ...e, on: !off.includes(e.action) })) })),
  }
}

/** Сколько событий группы включено — для подписи «5 из 7». */
export function groupCount(g: BotFeedGroup): { on: number; all: number } {
  return { on: g.events.filter((e) => e.on).length, all: g.events.length }
}
