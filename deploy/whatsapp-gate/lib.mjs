// Чистые правила шлюза (задача 052): какие события WhatsApp пропускать, номер ↔ jid, склейка
// сообщений подряд, пауза «печатает…». Проверяются `npm test` без сети.

/** «+996555123456» или «996555123456» → «996555123456@s.whatsapp.net»; не номер — null. */
export function jidOf(phone) {
  const digits = String(phone ?? '').replace(/\D/g, '')
  if (digits.length < 9 || digits.length > 15) return null
  return `${digits}@s.whatsapp.net`
}

/** Личный чат с человеком: не группа, не статусы, не рассылки и каналы. */
export function isPersonalChat(jid) {
  return typeof jid === 'string' && jid.endsWith('@s.whatsapp.net')
}

/** «996555123456@s.whatsapp.net» → «+996555123456». */
export function phoneOf(jid) {
  return `+${String(jid).split('@')[0].split(':')[0]}`
}

/** Текст сообщения; медиа, стикер, голосовое — null (ответим «напишите текстом»). */
export function textOf(message) {
  if (!message) return null
  const m = message.ephemeralMessage?.message ?? message.viewOnceMessage?.message ?? message
  if (typeof m.conversation === 'string') return m.conversation
  if (typeof m.extendedTextMessage?.text === 'string') return m.extendedTextMessage.text
  return null
}

/** Служебное событие (реакция, удаление, правка, ключи) — не сообщение клиента. */
export function isService(message) {
  if (!message) return true
  return Boolean(
    message.protocolMessage || message.reactionMessage || message.senderKeyDistributionMessage || message.messageContextInfo && Object.keys(message).length === 1,
  )
}

/** Пауза «печатает…» перед ответом: 1–4 с по длине текста. */
export function typingMs(text) {
  return Math.min(4000, Math.max(1000, String(text).length * 40))
}

/**
 * Склейка: несколько сообщений подряд за `ms` от одного номера — одно. `flush(from, text, media)`
 * вызывается один раз после тишины.
 */
export function makeDebouncer(ms, flush) {
  const pending = new Map()
  return {
    push(from, text, media) {
      const p = pending.get(from) ?? { parts: [], media: false, timer: null }
      if (text) p.parts.push(text)
      p.media = p.media || media
      clearTimeout(p.timer)
      p.timer = setTimeout(() => {
        pending.delete(from)
        flush(from, p.parts.join('\n'), p.media)
      }, ms)
      pending.set(from, p)
    },
    size: () => pending.size,
  }
}
