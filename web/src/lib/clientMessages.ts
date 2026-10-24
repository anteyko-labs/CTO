// Сообщения клиентам (SPEC-32): Телеграм и WhatsApp Business Platform — подписи журнала и проверка
// настройки шаблонов. Те же правила, что на сервере (`server/src/api/client_messages.rs`).

export type MessageKind = 'oil' | 'purchase'
export type MessageChannel = 'telegram' | 'whatsapp' | 'none'
export type MessageStatus = 'pending' | 'sent' | 'error' | 'not_sent'

export interface MessageSettings {
  whatsapp_enabled: boolean
  oil_template: string
  oil_language: string
  purchase_template: string
  purchase_language: string
  km_per_day: number
}

export interface MessageSettingsOut extends MessageSettings {
  whatsapp_configured: boolean
  telegram: boolean
  public_url: string | null
  oil_params: string[]
  purchase_params: string[]
}

export interface ClientMessage {
  id: string
  created_at: string
  sent_at: string | null
  kind: MessageKind
  test: boolean
  channel: MessageChannel
  recipient: string
  party_id: string | null
  party_name: string | null
  sale_number: number | null
  text: string
  template: string
  params: string[]
  status: MessageStatus
  error: string
}

export const KIND_LABELS: Record<MessageKind, string> = {
  oil: 'Замена масла',
  purchase: 'Закуп юрлица',
}

export const CHANNEL_LABELS: Record<MessageChannel, string> = {
  telegram: 'Телеграм',
  whatsapp: 'WhatsApp',
  none: '—',
}

export const STATUS_LABELS: Record<MessageStatus, string> = {
  pending: 'ждёт отправки',
  sent: 'отправлено',
  error: 'ошибка',
  not_sent: 'не отправлено',
}

export function statusTone(s: MessageStatus): 'green' | 'amber' | 'rose' | 'slate' {
  if (s === 'sent') return 'green'
  if (s === 'pending') return 'amber'
  if (s === 'error') return 'rose'
  return 'slate'
}

/** Имя шаблона — как в WhatsApp Manager: строчные латинские буквы, цифры и «_»; пусто — не задан. */
export function templateNameProblem(name: string): string | null {
  const n = name.trim()
  if (n === '') return null
  if (n.length > 512 || !/^[a-z0-9_]+$/.test(n)) return 'строчные латинские буквы, цифры и «_»'
  return null
}

/** Язык шаблона — код Meta: ru, en_US. */
export function languageProblem(lang: string): string | null {
  return /^[A-Za-z_]{2,10}$/.test(lang.trim()) ? null : 'код языка Meta, например ru'
}

/** «{{1}} — имя клиента» — переменные шаблона по порядку. */
export function paramLines(params: string[]): string[] {
  return params.map((p, i) => `{{${i + 1}}} — ${p}`)
}

/** Кому: чат или телефон. */
export function recipientText(m: Pick<ClientMessage, 'channel' | 'recipient'>): string {
  if (m.channel === 'telegram') return `чат ${m.recipient}`
  if (m.channel === 'whatsapp') return m.recipient
  return '—'
}

/** Текст шаблона «замена масла» для WhatsApp Manager (категория «Служебные», язык ru). */
export const OIL_TEMPLATE_TEXT =
  'Здравствуйте, {{1}}! Магазин Avtodom. Не забудьте поменять {{2}} на {{3}} на пробеге {{4}}. Бонусы нашего магазина и информация по ТО вашего авто — по ссылке: {{5}}\nЖдём вас в Avtodom!'

/** Текст шаблона «закуп юрлица». */
export const PURCHASE_TEMPLATE_TEXT =
  'Здравствуйте, {{1}}! {{2}} на сумму {{3}}. Водитель {{4}}, машина {{5}}. Общий долг на сегодня — {{6}}. Для полного ознакомления пройдите в личный кабинет: {{7}}\nСпасибо, что вы с нами!'
