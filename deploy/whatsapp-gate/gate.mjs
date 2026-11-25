// Шлюз WhatsApp по QR-коду (задача 052, ADR-217): номер магазина — связанное устройство, как
// WhatsApp Web. Слушает только 127.0.0.1; сервер Avtodom шлёт сюда текст и получает входящие.
//
// Переменные: GATE_KEY (обязателен, общий с сервером Avtodom — WA_GATE_KEY), GATE_BIND
// (127.0.0.1:8091), GATE_AUTH_DIR (папка сессии; QR заново после перезапуска не нужен),
// AVTODOM_URL (http://127.0.0.1:8080 — куда слать входящие), DEBOUNCE_MS (3000).
import { createServer } from 'node:http'
import { timingSafeEqual } from 'node:crypto'
import { mkdir, rm, readdir } from 'node:fs/promises'
import makeWASocket, { DisconnectReason, useMultiFileAuthState } from '@whiskeysockets/baileys'
import pino from 'pino'
import { isPersonalChat, isService, jidOf, makeDebouncer, phoneOf, textOf, typingMs } from './lib.mjs'

const KEY = process.env.GATE_KEY ?? process.env.WA_GATE_KEY ?? ''
if (KEY.length < 16) {
  console.error('GATE_KEY: не задан или короче 16 знаков')
  process.exit(1)
}
const [HOST, PORT] = (process.env.GATE_BIND ?? '127.0.0.1:8091').split(':')
const AUTH_DIR = process.env.GATE_AUTH_DIR ?? './session'
const AVTODOM = (process.env.AVTODOM_URL ?? 'http://127.0.0.1:8080').replace(/\/$/, '')
const DEBOUNCE_MS = Number(process.env.DEBOUNCE_MS ?? 3000)
const MAX_QR = 6

const log = pino({ level: process.env.GATE_LOG ?? 'warn' })
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

/** Состояние для экрана: off | qr | code | connecting | connected | logged_out. */
const st = { state: 'off', phone: '', since: null, qr: '', code: '', error: '' }
let sock = null
let qrCount = 0
let pairPhone = ''
const ourIds = new Set()

async function event(body) {
  try {
    await fetch(`${AVTODOM}/api/v1/whatsapp/gate-event`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', 'x-gate-key': KEY },
      body: JSON.stringify(body),
    })
  } catch (e) {
    log.warn({ e: String(e) }, 'avtodom недоступен')
  }
}

function setState(state, extra = {}) {
  const changed = st.state !== state
  Object.assign(st, { state, qr: '', code: '', error: '' }, extra)
  if (changed && ['connected', 'logged_out', 'off'].includes(state)) {
    void event({ type: 'state', state, phone: st.phone })
  }
}

const debouncer = makeDebouncer(DEBOUNCE_MS, (from, text, media) => void event({ type: 'message', from, text, media, from_me: false }))

async function start() {
  if (sock) return
  await mkdir(AUTH_DIR, { recursive: true })
  const { state: auth, saveCreds } = await useMultiFileAuthState(AUTH_DIR)
  setState('connecting')
  const s = makeWASocket({ auth, logger: log, printQRInTerminal: false, markOnlineOnConnect: false, syncFullHistory: false })
  sock = s
  s.ev.on('creds.update', saveCreds)
  s.ev.on('connection.update', async (u) => {
    if (u.qr) {
      qrCount += 1
      if (qrCount > MAX_QR) {
        stop()
        setState('off', { error: 'QR-код не отсканировали — нажмите «Подключить» ещё раз' })
        return
      }
      if (pairPhone && !s.authState.creds.registered) {
        try {
          const code = await s.requestPairingCode(pairPhone.replace(/\D/g, ''))
          setState('code', { code })
        } catch (e) {
          setState('off', { error: `код не получен: ${String(e)}` })
          stop()
        }
        pairPhone = ''
      } else if (st.state !== 'code') {
        setState('qr', { qr: u.qr })
      }
    }
    if (u.connection === 'open') {
      qrCount = 0
      setState('connected', { phone: phoneOf(s.user?.id ?? ''), since: new Date().toISOString() })
    }
    if (u.connection === 'close') {
      const code = u.lastDisconnect?.error?.output?.statusCode
      sock = null
      if (code === DisconnectReason.loggedOut) {
        await rm(AUTH_DIR, { recursive: true, force: true })
        setState('logged_out', { since: null })
      } else if (st.state !== 'off') {
        // Обрыв связи или перезапуск после первого входа — подключаемся снова сами.
        setTimeout(() => void start().catch((e) => log.error(e)), 3000)
      }
    }
  })
  s.ev.on('messages.upsert', ({ messages, type }) => {
    if (type !== 'notify') return
    for (const m of messages) {
      let jid = m.key?.remoteJid ?? ''
      const alt = m.key?.senderPn ?? m.key?.remoteJidAlt
      if (jid.endsWith('@lid') && alt) jid = alt
      if (!isPersonalChat(jid) || isService(m.message)) continue
      if (m.key.fromMe) {
        if (ourIds.delete(m.key.id)) continue
        // Ответил человек с телефона — сервер на время замолкает с этим клиентом.
        void event({ type: 'message', from: phoneOf(jid), text: textOf(m.message) ?? '', media: false, from_me: true })
        continue
      }
      const text = textOf(m.message)
      debouncer.push(phoneOf(jid), text, text === null)
    }
  })
}

function stop() {
  try {
    sock?.end(undefined)
  } catch {
    /* уже закрыт */
  }
  sock = null
}

async function send(to, text) {
  if (!sock || st.state !== 'connected') return [503, { error: 'WhatsApp не подключён', permanent: false }]
  const jid = jidOf(to)
  if (!jid) return [422, { error: 'неверный номер', permanent: true }]
  try {
    const [r] = await sock.onWhatsApp(jid)
    if (!r?.exists) return [422, { error: 'номера нет в WhatsApp', permanent: true }]
    await sock.presenceSubscribe(r.jid).catch(() => {})
    await sock.sendPresenceUpdate('composing', r.jid)
    await sleep(typingMs(text))
    await sock.sendPresenceUpdate('paused', r.jid)
    const sent = await sock.sendMessage(r.jid, { text })
    if (sent?.key?.id) {
      ourIds.add(sent.key.id)
      if (ourIds.size > 5000) ourIds.clear()
    }
    return [200, { ok: true, id: sent?.key?.id ?? '' }]
  } catch (e) {
    return [502, { error: `WhatsApp не принял: ${String(e?.message ?? e)}`, permanent: false }]
  }
}

function authorized(req) {
  const got = Buffer.from(String(req.headers.authorization ?? '').replace(/^Bearer /, ''))
  const want = Buffer.from(KEY)
  return got.length === want.length && timingSafeEqual(got, want)
}

async function body(req) {
  let raw = ''
  for await (const chunk of req) {
    raw += chunk
    if (raw.length > 64_000) throw new Error('слишком большой запрос')
  }
  return raw ? JSON.parse(raw) : {}
}

const server = createServer(async (req, res) => {
  const reply = (code, data) => {
    res.writeHead(code, { 'content-type': 'application/json; charset=utf-8' })
    res.end(JSON.stringify(data))
  }
  if (!authorized(req)) return reply(401, { error: 'ключ' })
  try {
    const url = new URL(req.url ?? '/', 'http://gate')
    if (req.method === 'GET' && url.pathname === '/status') return reply(200, st)
    if (req.method === 'POST' && url.pathname === '/connect') {
      const b = await body(req)
      qrCount = 0
      pairPhone = typeof b.phone === 'string' ? b.phone : ''
      if (sock && st.state !== 'connected') stop()
      await start()
      await sleep(1500)
      return reply(200, st)
    }
    if (req.method === 'POST' && url.pathname === '/logout') {
      try {
        await sock?.logout()
      } catch {
        /* уже вышли */
      }
      stop()
      await rm(AUTH_DIR, { recursive: true, force: true })
      setState('off', { phone: '', since: null })
      return reply(200, st)
    }
    if (req.method === 'POST' && url.pathname === '/send') {
      const b = await body(req)
      const [code, data] = await send(b.to, String(b.text ?? ''))
      return reply(code, data)
    }
    return reply(404, { error: 'нет такого' })
  } catch (e) {
    return reply(400, { error: String(e?.message ?? e) })
  }
})

server.listen(Number(PORT), HOST, async () => {
  log.warn(`шлюз WhatsApp на ${HOST}:${PORT}`)
  // Сессия уже есть — подключаемся сами, без QR.
  const files = await readdir(AUTH_DIR).catch(() => [])
  if (files.includes('creds.json')) await start().catch((e) => log.error(e))
})
