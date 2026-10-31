// Сообщения клиентам (SPEC-32): Телеграм и WhatsApp Business Platform — подписи журнала и проверка
// настройки шаблонов. Те же правила, что на сервере (`server/src/api/client_messages.rs`).

export type MessageKind = 'oil' | 'purchase' | 'payment' | 'monthly'
export type MessageChannel = 'telegram' | 'whatsapp' | 'none' | 'cabinet'
export type MessageStatus = 'pending' | 'sent' | 'error' | 'not_sent'

export interface MessageSettings {
  whatsapp_enabled: boolean
  oil_template: string
  oil_language: string
  purchase_template: string
  purchase_language: string
  km_per_day: number
  /** «Получили оплату» (задача 002, П7). */
  payment_enabled: boolean
  payment_text: string
  payment_template: string
  payment_language: string
  /** Раз в месяц — кто сколько должен (П7б). */
  monthly_enabled: boolean
  monthly_day: number
  monthly_hour: number
  monthly_text: string
  monthly_firm_text: string
  monthly_template: string
  monthly_language: string
  /** Напоминание о сроке долга в бот (П9). */
  debt_due_text: string
}

export interface MessageSettingsOut extends MessageSettings {
  whatsapp_configured: boolean
  telegram: boolean
  public_url: string | null
  oil_params: string[]
  purchase_params: string[]
  payment_params: string[]
  monthly_params: string[]
}

/** Подстановки текстов — те же списки, что на сервере (`PAYMENT_MARKS` и др.). */
export const PAYMENT_MARKS = ['имя', 'сумма', 'долг', 'дата'] as const
export const MONTHLY_MARKS = ['имя', 'долг', 'дата'] as const
export const MONTHLY_FIRM_MARKS = ['имя', 'долг', 'дата', 'ссылка'] as const
export const DEBT_DUE_MARKS = ['имя', 'долг', 'когда', 'дата'] as const

/** Подстановки текста, которых нет в списке: «телефон» из «{телефон}». */
export function unknownMarks(text: string, allowed: readonly string[]): string[] {
  const out: string[] = []
  for (const m of text.matchAll(/\{([^{}]*)\}/g)) {
    const mark = m[1] ?? ''
    if (!allowed.includes(mark) && !out.includes(mark)) out.push(mark)
  }
  return out
}

/** Ошибка текста для поля настройки или `null`. */
export function textProblem(text: string, allowed: readonly string[], optional = false): string | null {
  const t = text.trim()
  if (t === '') return optional ? null : 'текст не может быть пустым'
  if (t.length > 1000) return 'не длиннее 1000 знаков'
  const bad = unknownMarks(t, allowed)
  if (bad.length) return `непонятная подстановка ${bad.map((b) => `{${b}}`).join(', ')}`
  return null
}

/** Образец текста с примером подстановок — как увидит клиент (сервер делает то же для «Отправить себе»). */
export function sampleText(text: string, values: Record<string, string>): string {
  return text.replace(/\{([^{}]*)\}/g, (all, k: string) => values[k] ?? all)
}

/** Пример подстановок для образцов на экране. */
export const SAMPLE_VALUES: Record<string, string> = {
  имя: 'Азиз',
  сумма: '5 000 сом',
  долг: '1 240 сом',
  дата: '01.11.2026',
  когда: 'через 3 дня',
  ссылка: 'https://…/cabinet',
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
  payment: 'Получили оплату',
  monthly: 'Долг раз в месяц',
}

export const CHANNEL_LABELS: Record<MessageChannel, string> = {
  telegram: 'Телеграм',
  whatsapp: 'WhatsApp',
  none: '—',
  cabinet: 'Кабинет',
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

/** Текст шаблона «получили оплату» (юрлицам в WhatsApp, задача 002, П7). */
export const PAYMENT_TEMPLATE_TEXT =
  'Здравствуйте, {{1}}! Получили оплату {{2}}. Долг на сегодня — {{3}}.\nСпасибо, что вы с нами!'

/** Текст шаблона «долг раз в месяц» (П7б). */
export const MONTHLY_TEMPLATE_TEXT =
  'Здравствуйте, {{1}}! На {{2}} ваш долг — {{3}}. Подробно — в кабинете: {{4}}\nСпасибо, что вы с нами!'
